//! Device pairing (Phase 5.2).
//!
//! The Noise handshake leaves an encrypted channel with *someone*; pairing
//! is what turns it into an encrypted channel with *this specific device*.
//! Both sides show the same 6-digit code and the user confirms on both
//! screens — only then does the other side's public key get fixed in
//! `devices`.
//!
//! Hard rules:
//! - Pairing needs **both** sides to accept. Just one side seeing a
//!   different code is enough to abort.
//! - A different key for an already-known uid **gets rejected and flagged**.
//!   Trust is never silently re-established: that's exactly what a
//!   man-in-the-middle would do to impersonate one of your devices.
//! - An unpaired device receives no library data at all. The `Hello` with
//!   the counts comes after pairing, never before.

use crate::db;
use crate::engine::{self, is_disconnect};
use crate::wire::{Msg, Session};
use crate::AppState;
use anyhow::{anyhow, Result};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc::{channel, Sender};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

/// Every `library-changed` makes the frontend reload the ENTIRE library over
/// IPC. Emitting it per received file would mean, in a run of 100 files, 100
/// full reloads fighting for the same SQLite lock the transfer is using — on
/// the phone the app freezes and not even a screen can be opened. It's
/// emitted at most once per this interval; the end of the run always emits.
const LIBRARY_EVENT_MIN_MS: i64 = 1500;
static LAST_LIBRARY_EVENT: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

pub fn emit_library_changed(handle: &AppHandle, force: bool) {
    use std::sync::atomic::Ordering;
    let now = db::now_ms();
    if !force && now - LAST_LIBRARY_EVENT.load(Ordering::Relaxed) < LIBRARY_EVENT_MIN_MS {
        return;
    }
    LAST_LIBRARY_EVENT.store(now, Ordering::Relaxed);
    let _ = handle.emit("library-changed", ());
    // Files the sync just brought in have never been measured here.
    crate::kick_background(handle);
}

/// How long to wait for a person to look at the screen and confirm.
const DECISION_TIMEOUT: Duration = Duration::from_secs(120);

// Whatever doesn't need a screen lives in `sway_core::pairing` since Phase
// 6.1: keys, `devices` state, counts, and timeouts. What's left here is the
// ceremony — showing the code and waiting for someone to look at it — and
// the window events. The functions below are the same operation with the
// `AppHandle` plugged in.
use sway_core::pairing as core_pair;
use sway_core::pairing::{platform, Known, CONNECT_TIMEOUT, IO_TIMEOUT};

/// Pending pairing decisions, keyed by peer uid. The connection thread waits
/// on the receiver; the `confirm_pairing` command sends the answer.
///
/// Each entry carries the id of the request that put it there: a request
/// nobody saw expires after `DECISION_TIMEOUT`, and by then a newer one with
/// the same device may have taken its place. Cleaning up by uid alone
/// removed the newer one's sender, which then died in silence (Fase 1.9).
#[derive(Default)]
pub struct Pairing {
    pending: Mutex<HashMap<String, (u64, Sender<bool>)>>,
    slots: Mutex<Slots>,
    /// Rung when the running sync ends, for whoever is waiting its turn.
    freed: Condvar,
}

/// Who is syncing with this device, in either direction.
#[derive(Default)]
struct Slots {
    /// Devices with a sync running or waiting for its turn. With automatic
    /// sync there are several triggers (local change, peer showing up,
    /// periodic) that can fire almost together: two simultaneous runs
    /// against the same device would step on each other's half-downloaded
    /// files.
    active: HashSet<String>,
    /// The one actually running. **One at a time, counting both directions**
    /// (Fase 2.4): the per-device set alone let the phone pull from the
    /// server while the PC pushed to it, the same files arriving twice over
    /// two paths, one of them the slow one. The side being called used to be
    /// outside this entirely (`engine::serve_requests`); now it takes the
    /// slot too, and answers `Busy` if it can't.
    running: Option<String>,
}

fn slots(state: &AppState) -> MutexGuard<'_, Slots> {
    state.pairing.slots.lock().unwrap_or_else(PoisonError::into_inner)
}

/// How long an outgoing sync waits for its turn before giving up. Long on
/// purpose: what it waits for is a transfer, and a first sync of a whole
/// library takes a while. A hung session can't hold it that long —
/// `IO_TIMEOUT` cuts it first.
const SLOT_MAX_WAIT: Duration = Duration::from_secs(30 * 60);

/// Holds a sync slot; frees it on drop no matter what.
struct SyncGuard(AppHandle, String);

impl SyncGuard {
    /// For the side that calls: waits its turn. `None` if a sync with this
    /// same device is already running or waiting, or the turn never came.
    fn acquire(handle: &AppHandle, uid: &str, max_wait: Duration) -> Option<Self> {
        let state = handle.state::<AppState>();
        let mut s = slots(&state);
        if !s.active.insert(uid.to_string()) {
            return None; // one is already running with this peer
        }
        let deadline = Instant::now() + max_wait;
        if s.running.is_some() {
            log::info!("[sync] {uid} waits: another sync is running");
        }
        while s.running.is_some() {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                s.active.remove(uid);
                log::warn!("[sync] {uid}: gave up waiting for the running sync to end");
                return None;
            }
            s = state
                .pairing
                .freed
                .wait_timeout(s, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        s.running = Some(uid.to_string());
        Some(SyncGuard(handle.clone(), uid.to_string()))
    }

    /// For the side being called: doesn't wait. There's a connection open on
    /// the other end, and making it hang for minutes would only trip its
    /// timeouts — better to say `Busy` and let it come back.
    fn try_serve(handle: &AppHandle, uid: &str) -> Option<Self> {
        let state = handle.state::<AppState>();
        let mut s = slots(&state);
        if s.running.is_some() || s.active.contains(uid) {
            return None;
        }
        s.active.insert(uid.to_string());
        s.running = Some(uid.to_string());
        Some(SyncGuard(handle.clone(), uid.to_string()))
    }
}

impl Drop for SyncGuard {
    fn drop(&mut self) {
        let state = self.0.state::<AppState>();
        let mut s = slots(&state);
        s.active.remove(&self.1);
        if s.running.as_deref() == Some(self.1.as_str()) {
            s.running = None;
        }
        drop(s);
        state.pairing.freed.notify_all();
    }
}

/// The pairing with that device no longer works, and retrying won't fix it
/// (Fase 2.3): its key changed, or it rejects ours. Typed so callers can
/// stop insisting instead of retrying forever against it.
#[derive(Debug)]
pub struct PairingBroken(pub String);

impl std::fmt::Display for PairingBroken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PairingBroken {}

/// The other side is syncing with someone else (Fase 2.4). Not a failure:
/// come back in a moment.
#[derive(Debug)]
pub struct PeerBusy;

impl std::fmt::Display for PeerBusy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("busy syncing with another device")
    }
}

impl std::error::Error for PeerBusy {}

/// What's shown on a device whose key changed.
const KEY_CHANGED: &str = "its key changed - forget it and pair again if that was you";

impl Pairing {
    fn resolve(&self, uid: &str, accepted: bool) -> bool {
        match self.pending.lock().unwrap().remove(uid) {
            Some((_, tx)) => tx.send(accepted).is_ok(),
            None => false,
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PairingRequestEvent {
    uid: String,
    name: String,
    platform: String,
    code: String,
    /// `true` if the other device initiated the pairing.
    incoming: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PairingDoneEvent {
    uid: String,
    name: String,
    ok: bool,
    error: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerHelloEvent {
    pub uid: String,
    pub name: String,
    pub tracks: i64,
    pub playlists: i64,
    /// Clock difference with the other device, in ms. Matters for the LWW
    /// merge from 5.5: with clocks off, "last write wins" picks wrong.
    pub clock_skew_ms: i64,
}

// ---------------------------------------------------------------------------
// This device's cryptographic identity
// ---------------------------------------------------------------------------

fn private_key(handle: &AppHandle) -> Result<Vec<u8>> {
    let state = handle.state::<AppState>();
    let conn = state.db.lock().map_err(|_| anyhow!("db lock"))?;
    Ok(core_pair::keypair(&conn)?.0)
}

// ---------------------------------------------------------------------------
// `devices` state
// ---------------------------------------------------------------------------

fn known_state(handle: &AppHandle, uid: &str, pubkey: &[u8]) -> Known {
    let state = handle.state::<AppState>();
    let conn = match state.db.lock() {
        Ok(c) => c,
        Err(_) => return Known::Unknown,
    };
    core_pair::known_state(&conn, uid, pubkey)
}

/// Setting that says this device still has to be asked what to sync.
pub const SCOPE_PROMPT_KEY: &str = "scope_prompt_pending";

fn store_device(handle: &AppHandle, uid: &str, name: &str, platform: &str, pubkey: &[u8]) -> Result<()> {
    let state = handle.state::<AppState>();
    let conn = state.db.lock().map_err(|_| anyhow!("db lock"))?;
    let first = is_first_pairing(&conn)?;
    core_pair::store_device(&conn, uid, name, platform, pubkey)?;
    state.peers.clear_invalid(uid);
    if first {
        // The first link of a device used to start pulling the whole
        // library right away, with the scope still at its default of
        // "everything": picking playlists afterwards meant waiting for all of
        // it to arrive and then deleting by hand (Fase 1.8). Now it starts as
        // "selected" with nothing marked — which any version of Sway already
        // reads as "no files" — so the first sync only brings playlists and
        // metadata, exactly what's needed to choose from, and the Sync screen
        // asks the question.
        //
        // Only the app does this: the server shares `core_pair::store_device`
        // and is an archive, it takes everything.
        let me = db::this_device_uid(&conn)?;
        sway_core::scope::set_mode(&conn, &me, sway_core::scope::Mode::Selected)?;
        // What's already here is this device's own: it stays marked. Left
        // unmarked it would count as a playlist this device merely knows
        // about — not handed on, and dropped once no linked device has it
        // (`scope::prune_orphans`).
        let own: Vec<String> = conn
            .prepare("SELECT uid FROM playlists WHERE uid IS NOT NULL")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for uid in &own {
            sway_core::scope::set_playlist(&conn, &me, uid, true)?;
        }
        db::set_setting(&conn, SCOPE_PROMPT_KEY, "1")?;
    }
    Ok(())
}

/// Never linked to anything, and never made a scope choice. A device that
/// already chose (a row in `device_sync`) keeps its choice on a new link.
fn is_first_pairing(conn: &rusqlite::Connection) -> Result<bool> {
    let devices: i64 = conn.query_row("SELECT COUNT(*) FROM devices", [], |r| r.get(0))?;
    if devices > 0 {
        return Ok(false);
    }
    let me = db::this_device_uid(conn)?;
    let chosen: i64 = conn.query_row(
        "SELECT COUNT(*) FROM device_sync WHERE device_uid = ?1",
        [&me],
        |r| r.get(0),
    )?;
    Ok(chosen == 0)
}

fn library_counts(handle: &AppHandle) -> (i64, i64) {
    let state = handle.state::<AppState>();
    // The guard goes into its own variable: as a `match` binding it would be
    // a temporary that outlives `state`, and it wouldn't compile.
    let db = state.db.lock();
    match db {
        Ok(conn) => core_pair::library_counts(&conn),
        Err(_) => (0, 0),
    }
}

fn me(handle: &AppHandle) -> Result<(String, String)> {
    let state = handle.state::<AppState>();
    let conn = state.db.lock().map_err(|_| anyhow!("db lock"))?;
    core_pair::me(&conn)
}

// ---------------------------------------------------------------------------
// User confirmation
// ---------------------------------------------------------------------------

/// How the wait for the person ended.
enum Decision {
    /// The person here answered (or nobody did before the timeout: `false`).
    Here(bool),
    /// The other side sent something first: its `Reject` because its own
    /// dialog was closed, or its answer. It hasn't been read yet.
    PeerSpoke,
}

/// How often the wait looks at the socket for a message from the peer.
const WATCH_EVERY: Duration = Duration::from_millis(250);

/// Shows the code and waits for the person to decide. The timeout prevents a
/// thread (and a connection) from being left hanging if nobody looks at the
/// screen.
///
/// With `watch`, the wait also ends as soon as the peer sends something:
/// otherwise a dialog closed on the other device left this one showing a
/// dead code for up to two minutes.
fn ask_user(handle: &AppHandle, ev: PairingRequestEvent, watch: Option<&Session>) -> Decision {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = channel();
    {
        let state = handle.state::<AppState>();
        state.pairing.pending.lock().unwrap().insert(ev.uid.clone(), (id, tx));
    }
    let uid = ev.uid.clone();
    let _ = handle.emit("pairing-request", ev);
    let deadline = Instant::now() + DECISION_TIMEOUT;
    let decision = loop {
        match rx.recv_timeout(WATCH_EVERY) {
            Ok(accepted) => break Decision::Here(accepted),
            Err(RecvTimeoutError::Disconnected) => break Decision::Here(false),
            Err(RecvTimeoutError::Timeout) => {
                if watch.is_some_and(|s| s.peer_has_spoken()) {
                    break Decision::PeerSpoke;
                }
                if Instant::now() >= deadline {
                    break Decision::Here(false);
                }
            }
        }
    };
    let state = handle.state::<AppState>();
    let mut pending = state.pairing.pending.lock().unwrap();
    // Only our own entry: a newer request for the same device may be there.
    if pending.get(&uid).is_some_and(|(owner, _)| *owner == id) {
        pending.remove(&uid);
    }
    decision
}

/// Called by the `confirm_pairing` command from the UI.
pub fn resolve_decision(handle: &AppHandle, uid: &str, accepted: bool) -> bool {
    handle.state::<AppState>().pairing.resolve(uid, accepted)
}

// ---------------------------------------------------------------------------
// Side that accepts connections
// ---------------------------------------------------------------------------

/// Listens on the port 5.1 reserved and announced via mDNS.
pub fn spawn_server(handle: AppHandle, listener: TcpListener) {
    std::thread::spawn(move || {
        log::info!("[pair] listening on {:?}", listener.local_addr());
        for stream in listener.incoming() {
            let stream = match stream {
                Ok(s) => s,
                Err(e) => {
                    log::warn!("[pair] accept failed: {e}");
                    continue;
                }
            };
            let handle = handle.clone();
            std::thread::spawn(move || {
                if let Err(e) = serve(&handle, stream) {
                    // Reachability probes (see discovery::spawn_prober)
                    // connect and disconnect without sending anything: it's
                    // expected traffic, not an error worth reporting every
                    // 10 seconds.
                    if is_disconnect(&e) {
                        log::debug!("[pair] reachability probe");
                    } else {
                        log::warn!("[pair] incoming connection ended: {e}");
                    }
                }
            });
        }
    });
}

fn serve(handle: &AppHandle, stream: TcpStream) -> Result<()> {
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let private = private_key(handle)?;
    let mut sess = Session::accept(stream, &private)?;

    match sess.recv()? {
        // The token a `PairRequest` carries is for devices without a screen.
        // There's one here: the proof comes from the person comparing the
        // code, so the token is ignored even if it's present.
        Msg::PairRequest { uid, name, platform, token: _ } => {
            match known_state(handle, &uid, &sess.peer_pubkey) {
                Known::KeyMismatch => {
                    let _ = sess.send(&Msg::Reject {
                        reason: "different key from the one you already had for this device".into(),
                    });
                    warn_key_mismatch(handle, &uid, &name);
                    return Err(anyhow!("different key for {uid}"));
                }
                Known::Trusted | Known::Unknown => {}
            }

            let decision = ask_user(
                handle,
                PairingRequestEvent {
                    uid: uid.clone(),
                    name: name.clone(),
                    platform: platform.clone(),
                    code: sess.code.clone(),
                    incoming: true,
                },
                Some(&sess),
            );
            let accepted_here = match decision {
                Decision::Here(accepted) => accepted,
                // The caller says nothing until it hears our answer, except
                // to give up: its dialog was closed or rejected.
                Decision::PeerSpoke => {
                    let reason = match sess.recv() {
                        Ok(Msg::Reject { reason }) => reason,
                        _ => "cancelled on the other device".to_string(),
                    };
                    emit_done(handle, &uid, &name, false, Some(&reason));
                    return Ok(());
                }
            };
            sess.send(&Msg::PairResponse {
                accepted: accepted_here,
            })?;
            if !accepted_here {
                emit_done(handle, &uid, &name, false, Some("rejected on this device"));
                return Ok(());
            }
            // The other side also has to have accepted.
            let accepted_there = match sess.recv()? {
                Msg::PairAck { accepted } => accepted,
                Msg::Reject { reason } => {
                    emit_done(handle, &uid, &name, false, Some(&reason));
                    return Ok(());
                }
                other => return Err(anyhow!("expected PairAck, got {other:?}")),
            };
            if !accepted_there {
                emit_done(handle, &uid, &name, false, Some("rejected on the other device"));
                return Ok(());
            }
            store_device(handle, &uid, &name, &platform, &sess.peer_pubkey)?;
            emit_done(handle, &uid, &name, true, None);
            let _ = handle.emit("peers-changed", ());
            exchange_hello(handle, &mut sess, &uid, &name)
        }
        Msg::Hello {
            uid,
            name,
            tracks,
            playlists,
            clock_ms,
            ..
        } => {
            match known_state(handle, &uid, &sess.peer_pubkey) {
                Known::Trusted => {}
                Known::KeyMismatch => {
                    let _ = sess.send(&Msg::Reject {
                        reason: "different key from the one you already had for this device".into(),
                    });
                    // Nobody here asked for this connection: no dialog, just
                    // the row marked as broken.
                    key_mismatch(handle, &uid, &name);
                    return Err(anyhow!("different key for {uid}"));
                }
                Known::Unknown => {
                    // Not an error on the other side: we probably unpaired
                    // it. Let it know so it can clear its row.
                    let _ = sess.send(&Msg::NotPaired);
                    return Ok(());
                }
            }
            pairing_ok(handle, &uid);
            let Some(_slot) = SyncGuard::try_serve(handle, &uid) else {
                log::info!("[sync] {name} wants to sync, but another sync is running: busy");
                let _ = sess.send(&Msg::Busy);
                return Ok(());
            };
            let (my_uid, my_name) = me(handle)?;
            let (my_tracks, my_playlists) = library_counts(handle);
            sess.send(&Msg::Hello {
                uid: my_uid,
                name: my_name,
                platform: platform(),
                tracks: my_tracks,
                playlists: my_playlists,
                clock_ms: db::now_ms(),
            })?;
            report_hello(handle, &uid, &name, tracks, playlists, clock_ms);
            // The totals are useful to whoever has no screen (the server,
            // see `crates/server/src/serve.rs`); here what happened has
            // already gone out through the progress events.
            let served = engine::serve_requests(&crate::AppHost(handle), &mut sess, &uid);
            // The other side ran this one: its end is the session's end.
            crate::sync_notice::end(handle, &uid);
            served.map(|_| ())
        }
        // The other side removed us from its devices. Only accepted if its
        // key is the one we had stored — i.e. if the handshake proved it's
        // really them and not someone asking to unpair us.
        Msg::Unpair { uid } => {
            match known_state(handle, &uid, &sess.peer_pubkey) {
                Known::Trusted => {
                    forget_device(handle, &uid)?;
                    log::info!("[pair] {uid} unpaired us");
                    let _ = handle.emit("peers-changed", ());
                    Ok(())
                }
                _ => Err(anyhow!("unpair from a device that is not paired ({uid})")),
            }
        }
        other => Err(anyhow!("unexpected first message: {other:?}")),
    }
}

// ---------------------------------------------------------------------------
// Calling side
// ---------------------------------------------------------------------------

/// Connects to a peer: pairs it if needed, and if already paired exchanges
/// `Hello`. Runs on its own thread — the other side might have a person
/// taking a while to confirm.
pub fn connect_peer(handle: AppHandle, uid: String) {
    std::thread::spawn(move || {
        let name = peer_name(&handle, &uid);
        if let Err(e) = connect_inner(&handle, &uid) {
            log::warn!("[pair] connection with {uid} failed: {e}");
            // Only NETWORK failures grey out the row: saying "connected"
            // right after a timeout is the worst possible combination. A
            // logical rejection (not paired, different key) doesn't mean the
            // device isn't there — greying it out would be lying too, just
            // in the other direction.
            if e.downcast_ref::<std::io::Error>().is_some() {
                let state = handle.state::<AppState>();
                if state.peers.mark_unreachable(&uid) {
                    let _ = handle.emit("peers-changed", ());
                }
            }
            emit_done(&handle, &uid, &name, false, Some(&e.to_string()));
        }
    });
}

pub(crate) fn peer_name(handle: &AppHandle, uid: &str) -> String {
    handle
        .state::<AppState>()
        .peers
        .list()
        .into_iter()
        .find(|p| p.uid == uid)
        .map(|p| p.name)
        .unwrap_or_else(|| uid.to_string())
}

fn peer_addr(handle: &AppHandle, uid: &str) -> Result<SocketAddr> {
    let peer = handle
        .state::<AppState>()
        .peers
        .list()
        .into_iter()
        .find(|p| p.uid == uid)
        .ok_or_else(|| anyhow!("that device is no longer visible on the network"))?;
    let addr = peer
        .addrs
        .first()
        .ok_or_else(|| anyhow!("that device did not publish any address"))?;
    crate::discovery::resolve(addr, peer.port)
        .ok_or_else(|| anyhow!("could not resolve the address ({addr}:{})", peer.port))
}

fn connect_inner(handle: &AppHandle, uid: &str) -> Result<()> {
    let addr = peer_addr(handle, uid)?;
    let name = peer_name(handle, uid);
    let stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let private = private_key(handle)?;
    let mut sess =
        Session::connect(stream, &private).map_err(|e| session_failed(handle, uid, e))?;

    let (my_uid, my_name) = me(handle)?;
    match known_state(handle, uid, &sess.peer_pubkey) {
        Known::KeyMismatch => {
            warn_key_mismatch(handle, uid, &name);
            return Err(anyhow!(
                "the key of {name} does not match the one you had stored"
            ));
        }
        Known::Trusted => {
            let (tracks, playlists) = library_counts(handle);
            sess.send(&Msg::Hello {
                uid: my_uid,
                name: my_name,
                platform: platform(),
                tracks,
                playlists,
                clock_ms: db::now_ms(),
            })?;
            match sess.recv()? {
                Msg::Hello {
                    uid: their_uid,
                    name: their_name,
                    tracks,
                    playlists,
                    clock_ms,
                    ..
                } => {
                    report_hello(handle, &their_uid, &their_name, tracks, playlists, clock_ms);
                    Ok(())
                }
                // We got unpaired from the other side. It's credible: the
                // handshake already proved the key is the one we had
                // stored. Still showing "Paired" would be a lie.
                Msg::NotPaired => {
                    forget_device(handle, uid)?;
                    let _ = handle.emit("peers-changed", ());
                    Err(anyhow!("{name} no longer has you paired"))
                }
                Msg::Busy => Err(anyhow!("{name} is syncing with another device - try again in a moment")),
                Msg::Reject { reason } => {
                    mark_broken(handle, uid, &format!("it rejects this device: {reason}"));
                    Err(anyhow!(reason))
                }
                other => Err(anyhow!("expected Hello, got {other:?}")),
            }
        }
        Known::Unknown => {
            sess.send(&Msg::PairRequest {
                uid: my_uid,
                name: my_name,
                platform: platform(),
                // Pairing with another device that has a screen: the
                // six-digit code is enough. The token is for the server
                // (Phase 6.3).
                token: None,
            })?;
            let ev = PairingRequestEvent {
                uid: uid.to_string(),
                name: name.clone(),
                platform: String::new(),
                code: sess.code.clone(),
                incoming: false,
            };
            // The other side may answer before the person here does. A "no"
            // ends it right away; a "yes" is kept and the wait goes on,
            // without watching: the peer is silent until our `PairAck`.
            let mut accepted_there = None;
            let accepted_here = match ask_user(handle, ev.clone(), Some(&sess)) {
                Decision::Here(accepted) => accepted,
                Decision::PeerSpoke => match sess.recv()? {
                    Msg::PairResponse { accepted: true } => {
                        accepted_there = Some(true);
                        match ask_user(handle, ev, None) {
                            Decision::Here(accepted) => accepted,
                            Decision::PeerSpoke => false,
                        }
                    }
                    Msg::PairResponse { accepted: false } => {
                        emit_done(handle, uid, &name, false, Some("rejected on the other device"));
                        return Ok(());
                    }
                    Msg::Reject { reason } => {
                        emit_done(handle, uid, &name, false, Some(&reason));
                        return Ok(());
                    }
                    other => return Err(anyhow!("expected PairResponse, got {other:?}")),
                },
            };
            if !accepted_here {
                // Cut it off here without waiting for the other side's
                // answer: there could be someone looking at the screen until
                // the timeout expires.
                let _ = sess.send(&Msg::Reject {
                    reason: "rejected on the other device".into(),
                });
                emit_done(handle, uid, &name, false, Some("rejected on this device"));
                return Ok(());
            }
            let accepted_there = match accepted_there {
                Some(accepted) => accepted,
                None => match sess.recv()? {
                    Msg::PairResponse { accepted } => accepted,
                    Msg::Reject { reason } => {
                        emit_done(handle, uid, &name, false, Some(&reason));
                        return Ok(());
                    }
                    other => return Err(anyhow!("expected PairResponse, got {other:?}")),
                },
            };
            sess.send(&Msg::PairAck {
                accepted: accepted_here,
            })?;
            if !accepted_there {
                emit_done(handle, uid, &name, false, Some("rejected on the other device"));
                return Ok(());
            }
            store_device(handle, uid, &name, &platform_of(handle, uid), &sess.peer_pubkey)?;
            emit_done(handle, uid, &name, true, None);
            let _ = handle.emit("peers-changed", ());
            exchange_hello(handle, &mut sess, uid, &name)?;
            drop(sess);
            // Nothing else starts the first sync of a new link: discovery
            // saw this device before it was paired (and skipped it), and the
            // probe only reacts to offline -> online. Without this the
            // playlists to choose from (1.8) took up to the periodic pass to
            // show up. Only the side that asked does it, so the two don't
            // start one against each other.
            crate::autosync::peer_came_online(handle, uid);
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// File server (Phase 6.3)
// ---------------------------------------------------------------------------

/// How long to wait for the server. `IO_TIMEOUT`'s 180 s are for the other
/// case, where a person might be deciding; here a machine answers, and this
/// call blocks the screen in the meantime.
const SERVER_IO_TIMEOUT: Duration = Duration::from_secs(15);

/// Pairs with a file server, which isn't discovered on its own and has no
/// screen to compare a code on.
///
/// Returns the name the server declared. Unlike pairing between devices,
/// this does NOT return right away: there's no one on the other side who
/// might take a while to decide, so the response arrives on the same trip
/// and the UI can show the result without waiting for an event.
pub fn pair_with_server(handle: &AppHandle, host: &str, port: u16, token: &str) -> Result<String> {
    let (host, port) = split_host_port(host, port);
    let host = host.as_str();
    let addr = crate::discovery::resolve(host, port)
        .ok_or_else(|| anyhow!("could not resolve {host}:{port}"))?;
    let stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)?;
    stream.set_read_timeout(Some(SERVER_IO_TIMEOUT))?;
    stream.set_write_timeout(Some(SERVER_IO_TIMEOUT))?;
    let private = private_key(handle)?;
    // No uid yet to mark: the version problem goes straight to the dialog.
    let mut sess = Session::connect(stream, &private).map_err(|e| {
        match e.downcast_ref::<sway_core::wire::VersionMismatch>() {
            Some(v) => anyhow!(crate::discovery::update_note(
                "The server",
                core_pair::PLATFORM_SERVER,
                v.peer_is_older,
                &v.their_app,
            )),
            None => e,
        }
    })?;

    let (my_uid, my_name) = me(handle)?;
    sess.send(&Msg::PairRequest {
        uid: my_uid,
        name: my_name,
        platform: platform(),
        token: Some(token.to_string()),
    })?;
    match sess.recv()? {
        Msg::PairResponse { accepted: true } => {}
        Msg::PairResponse { accepted: false } => {
            return Err(anyhow!("the server rejected the token"))
        }
        Msg::Reject { reason } => return Err(anyhow!(reason)),
        other => return Err(anyhow!("expected PairResponse, got {other:?}")),
    }
    sess.send(&Msg::PairAck { accepted: true })?;

    // Only here do we learn which uid we're talking to: a server is called
    // by address, not by identity. That's why the key check comes after and
    // not before — but it does come, and before anything is stored.
    let (their_uid, their_name, their_platform) = match sess.recv()? {
        Msg::Hello {
            uid, name, platform, ..
        } => (uid, name, platform),
        Msg::Reject { reason } => return Err(anyhow!(reason)),
        other => return Err(anyhow!("expected Hello, got {other:?}")),
    };
    if let Known::KeyMismatch = known_state(handle, &their_uid, &sess.peer_pubkey) {
        warn_key_mismatch(handle, &their_uid, &their_name);
        return Err(anyhow!(
            "the key of {their_name} does not match the one you had stored"
        ));
    }

    let (tracks, playlists) = library_counts(handle);
    let (my_uid, my_name) = me(handle)?;
    sess.send(&Msg::Hello {
        uid: my_uid,
        name: my_name,
        platform: platform(),
        tracks,
        playlists,
        clock_ms: db::now_ms(),
    })?;

    store_device(handle, &their_uid, &their_name, &their_platform, &sess.peer_pubkey)?;
    let address = format!("{host}:{port}");
    let stale: Vec<String> = {
        let state = handle.state::<AppState>();
        let conn = state.db.lock().map_err(|_| anyhow!("db lock"))?;
        core_pair::set_device_address(&conn, &their_uid, &address)?;
        // Another device at this same address: it's this server from before
        // it was reinstalled (new keys, new uid). That row can never
        // authenticate again, and it used to stay forever next to the new
        // one — red, retrying, resurrected on every startup (Fase 2.3).
        let stale: Vec<String> = core_pair::devices_with_address(&conn)
            .into_iter()
            .filter(|(uid, _, _, addr)| uid != &their_uid && addr == &address)
            .map(|(uid, ..)| uid)
            .collect();
        for uid in &stale {
            core_pair::forget_device(&conn, uid)?;
        }
        stale
    };
    for uid in stale {
        log::info!("[pair] {uid} was the previous identity of {their_name}: forgotten");
        let state = handle.state::<AppState>();
        state.peers.forget(&uid);
        state.watchers.wake(&uid);
    }
    // Enters the same list as mDNS peers: from here down it's just another
    // device.
    handle
        .state::<AppState>()
        .peers
        .add_manual(&their_uid, &their_name, &their_platform, host, port);
    let _ = handle.emit("peers-changed", ());
    log::info!("[pair] server {their_name} paired at {host}:{port}");
    Ok(their_name)
}

/// Puts the fixed-address devices back on the list. The LAN ones are
/// restored by mDNS on its own; nobody announces these, so without this they
/// disappear on every startup.
pub fn restore_manual_peers(handle: &AppHandle) {
    let state = handle.state::<AppState>();
    let Ok(conn) = state.db.lock() else { return };
    for (uid, name, platform, address) in core_pair::devices_with_address(&conn) {
        let Some((host, port)) = split_addr(&address) else {
            log::warn!("[pair] invalid stored address for {name}: {address}");
            continue;
        };
        state.peers.add_manual(&uid, &name, &platform, host, port);
    }
}

/// What the person typed, turned into something resolvable.
///
/// The field asks for a name or an IP, but what people have at hand is the
/// server's URL, and pasting it is what anyone would do. Without this,
/// `https://home.com/` would get passed as-is to name resolution and fail
/// with a "could not resolve" that looked like a DNS problem.
///
/// If the text carries its own port, that one wins: it's more specific than
/// the one in the field next to it, and it's the one the person just read
/// somewhere.
fn split_host_port(input: &str, default_port: u16) -> (String, u16) {
    let mut s = input.trim();
    // Scheme: `https://`, `http://`, `sway://`, whatever.
    if let Some((_, rest)) = s.split_once("://") {
        s = rest;
    }
    // Credentials, in case they came pasted from a URL.
    if let Some((_, rest)) = s.rsplit_once('@') {
        s = rest;
    }
    // Path, query, or fragment: none of that is part of the host.
    s = s.split(['/', '?', '#']).next().unwrap_or(s);

    // IPv6 in brackets: `[fd00::1]:7420`.
    if let Some(rest) = s.strip_prefix('[') {
        if let Some((addr, tail)) = rest.split_once(']') {
            let port = tail.strip_prefix(':').and_then(|p| p.parse().ok());
            return (addr.to_string(), port.unwrap_or(default_port));
        }
    }
    // A single `:` is host:port. Several are an IPv6 written without
    // brackets, and there's no port to split off there.
    if s.matches(':').count() == 1 {
        if let Some((h, p)) = s.split_once(':') {
            // A port that isn't a number is a typo. The host is kept and the
            // one from the field is used: better to try than to fail with a
            // "could not resolve home.example:port", which sends people off
            // to check DNS.
            return (h.to_string(), p.parse().unwrap_or(default_port));
        }
    }
    (s.to_string(), default_port)
}

/// `host:port`, with the host possibly being a name or an IPv6 in brackets.
fn split_addr(address: &str) -> Option<(&str, u16)> {
    let (host, port) = address.rsplit_once(':')?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    Some((host, port.parse().ok()?))
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPlanEvent {
    pub uid: String,
    pub name: String,
    pub plan: crate::manifest::Plan,
    pub bytes_in: i64,
    pub bytes_out: i64,
}

/// Sync dry run (Phase 5.3): asks the other side for its inventory, compares
/// it with its own, and publishes what would happen. **Writes nothing.**
pub fn preview_sync(handle: AppHandle, uid: String) {
    std::thread::spawn(move || {
        let name = peer_name(&handle, &uid);
        match preview_inner(&handle, &uid) {
            Ok(plan) => {
                if plan.is_empty() {
                    log::info!("[sync] {name}: nothing to sync");
                }
                let _ = handle.emit(
                    "sync-plan",
                    SyncPlanEvent {
                        uid: uid.clone(),
                        name,
                        bytes_in: plan.bytes_in(),
                        bytes_out: plan.bytes_out(),
                        plan,
                    },
                );
            }
            Err(e) => {
                log::warn!("[sync] preview with {uid} failed: {e}");
                emit_done(&handle, &uid, &name, false, Some(&e.to_string()));
            }
        }
    });
}

/// Opens a session with an already paired device and introduces itself.
fn open_session(handle: &AppHandle, uid: &str) -> Result<Session> {
    open_session_with(handle, uid, IO_TIMEOUT)
}

/// Same, with the read timeout chosen by the caller.
///
/// Needed by the connection that waits for updates (`watch.rs`): there,
/// silence is normal, not a symptom, and `IO_TIMEOUT` would cut it off
/// between two heartbeats.
pub(crate) fn open_session_with(
    handle: &AppHandle,
    uid: &str,
    read_timeout: Duration,
) -> Result<Session> {
    let addr = peer_addr(handle, uid)?;
    let stream = match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
        Ok(s) => s,
        // Nobody answers: say so now. Before, only the probe could turn it
        // grey, and it skips devices with a watcher — a server that went
        // away stayed green (Fase 2.1).
        Err(e) => {
            let state = handle.state::<AppState>();
            if state.peers.mark_unreachable(uid) {
                let _ = handle.emit("peers-changed", ());
            }
            return Err(e.into());
        }
    };
    stream.set_read_timeout(Some(read_timeout))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let private = private_key(handle)?;
    let mut sess =
        Session::connect(stream, &private).map_err(|e| session_failed(handle, uid, e))?;

    match known_state(handle, uid, &sess.peer_pubkey) {
        Known::Trusted => {}
        Known::KeyMismatch => {
            key_mismatch(handle, uid, &peer_name(handle, uid));
            return Err(PairingBroken(KEY_CHANGED.into()).into());
        }
        Known::Unknown => return Err(anyhow!("not paired yet")),
    }

    let (my_uid, my_name) = me(handle)?;
    let (tracks, playlists) = library_counts(handle);
    sess.send(&Msg::Hello {
        uid: my_uid,
        name: my_name,
        platform: platform(),
        tracks,
        playlists,
        clock_ms: db::now_ms(),
    })?;
    match sess.recv()? {
        Msg::Hello { .. } => {
            pairing_ok(handle, uid);
            Ok(sess)
        }
        Msg::NotPaired => {
            forget_device(handle, uid)?;
            let _ = handle.emit("peers-changed", ());
            Err(anyhow!("that device no longer has you paired"))
        }
        Msg::Busy => Err(PeerBusy.into()),
        // The other side has a different key stored for us.
        Msg::Reject { reason } => {
            mark_broken(handle, uid, &format!("it rejects this device: {reason}"));
            Err(PairingBroken(reason).into())
        }
        other => Err(anyhow!("expected Hello, got {other:?}")),
    }
}

/// How long the Refresh button waits for each device to answer.
const VERIFY_TIMEOUT: Duration = Duration::from_secs(5);

/// The Refresh button: brings everything that otherwise updates over time
/// up to date now.
///
/// It used to only re-ask mDNS and probe the port, and that left out the
/// states that matter most: a server whose token changed kept showing as
/// paired until the watcher happened to reconnect, and a device with a
/// standing connection wasn't probed at all. Now it also opens a session
/// with every paired device that answers — the only way to learn that the
/// other side unpaired us (`NotPaired`), changed its key, or rejects ours —
/// and wakes the watchers, so a server that just came back is watched again
/// right away instead of after its backoff.
pub fn check_now(handle: &AppHandle) {
    if let Err(e) = crate::discovery::refresh(handle) {
        log::debug!("[pair] refresh: mDNS query failed: {e}");
    }
    crate::discovery::probe_all(handle);
    let reachable: Vec<String> = {
        let state = handle.state::<AppState>();
        let Ok(conn) = state.db.lock() else { return };
        state
            .peers
            .merged_list(&conn)
            .into_iter()
            .filter(|p| p.paired && p.online)
            .map(|p| p.uid)
            .collect()
    };
    let checks: Vec<_> = reachable
        .into_iter()
        .map(|uid| {
            let handle = handle.clone();
            std::thread::spawn(move || verify_pairing(&handle, &uid))
        })
        .collect();
    for c in checks {
        let _ = c.join();
    }
    let _ = handle.emit("peers-changed", ());
}

/// Says hello and leaves. Everything worth knowing is recorded on the way by
/// `open_session_with`: unreachable, broken, unpaired, or fine.
fn verify_pairing(handle: &AppHandle, uid: &str) {
    handle.state::<AppState>().watchers.wake(uid);
    match open_session_with(handle, uid, VERIFY_TIMEOUT) {
        Ok(mut sess) => {
            let _ = sess.send(&Msg::Bye);
        }
        // Busy syncing with someone else still means paired and there.
        Err(e) => log::debug!("[pair] refresh: {uid}: {e}"),
    }
}

fn preview_inner(handle: &AppHandle, uid: &str) -> Result<crate::manifest::Plan> {
    let mut sess = open_session(handle, uid)?;
    let (mut plan, _, dir) = engine::fetch_plan(&crate::AppHost(handle), &mut sess)?;
    engine::restrict(&mut plan, dir);
    let _ = sess.send(&Msg::Bye);
    Ok(plan)
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SyncDoneEvent {
    uid: String,
    name: String,
    received: usize,
    sent: usize,
    failed: usize,
    bytes: u64,
    /// Organization records (metadata, playlists, memberships) applied
    /// between the two sides. Without this, a sync that only moved
    /// playlists would report "nothing to transfer", which is false.
    organized: usize,
    /// Triggered by automatic sync, not the user. The UI doesn't notify for
    /// automatic runs that did nothing: that would be a banner every few
    /// minutes.
    auto: bool,
    error: Option<String>,
}

/// Runs the file transfer for the plan (Phase 5.4).
///
/// Files only: metadata, playlists, and deletions are 5.5 and 5.6. A file
/// that fails doesn't stop the run — it's counted and the run continues with
/// the next one, because stopping halfway over one unreadable file would be
/// worse than finishing with something missing.
pub fn sync_files(handle: AppHandle, uid: String) {
    run_sync(handle, uid, false)
}

/// Same, but triggered by automatic sync.
pub fn sync_files_auto(handle: AppHandle, uid: String) {
    run_sync(handle, uid, true)
}

/// Waits for syncs with those devices to finish.
///
/// This is what always keeps the server after the local network: the LAN
/// moves files fast and without internet, and when it's later the server's
/// turn, the inventory already counts what just arrived — so it only asks
/// for what's really missing and nothing travels twice.
///
/// The initial breathing room isn't wasted: `sync_files_auto` only takes its
/// mark inside the thread it launches, so asking right away would find
/// everything quiet and the wait would wait for nothing.
pub fn wait_until_idle(handle: &AppHandle, uids: &[String], max: Duration) {
    const GRACE: Duration = Duration::from_millis(800);
    const POLL: Duration = Duration::from_millis(250);
    std::thread::sleep(GRACE);
    let until = std::time::Instant::now() + max;
    while std::time::Instant::now() < until {
        let busy = {
            let state = handle.state::<AppState>();
            let s = slots(&state);
            uids.iter().any(|u| s.active.contains(u))
        };
        if !busy {
            return;
        }
        std::thread::sleep(POLL);
    }
    log::warn!("[autosync] local network is still busy: syncing with the server anyway");
}

fn run_sync(handle: AppHandle, uid: String, auto: bool) {
    std::thread::spawn(move || run_sync_blocking(handle, uid, auto));
}

/// Same, on the caller's own thread.
///
/// Needed by the watcher (`watch.rs`): it has to catch up **before** it
/// starts waiting for updates, and if it doesn't wait for the run to finish
/// it would end up waiting against a state it's about to change itself.
pub(crate) fn run_sync_blocking(handle: AppHandle, uid: String, auto: bool) {
    let started = Instant::now();
    let mut backoff = BUSY_RETRY_MIN;
    loop {
        let Some(guard) = SyncGuard::acquire(&handle, &uid, SLOT_MAX_WAIT) else {
            log::debug!("[sync] a sync with {uid} is already running");
            return;
        };
        let name = peer_name(&handle, &uid);
        let result = sync_inner(&handle, &uid);
        crate::sync_notice::end(&handle, &uid);
        // The other side is syncing with someone else. Our slot goes back
        // before waiting: holding it would make that someone get `Busy` from
        // us too, and both would wait on each other.
        if result.as_ref().is_err_and(|e| e.is::<PeerBusy>())
            && started.elapsed() < BUSY_GIVE_UP
        {
            drop(guard);
            log::info!("[sync] {name} is busy with another device: retrying");
            std::thread::sleep(backoff + jitter());
            backoff = (backoff * 2).min(BUSY_RETRY_MAX);
            continue;
        }
        match result {
            Ok(engine::SyncResult { received, sent, failed, bytes, organized }) => {
                log::info!(
                    "[sync] {name}: {received} received, {sent} sent, {failed} failed, {organized} organization"
                );
                let _ = handle.emit(
                    "sync-done",
                    SyncDoneEvent {
                        uid: uid.clone(),
                        name,
                        received,
                        sent,
                        failed,
                        bytes,
                        organized,
                        auto,
                        error: None,
                    },
                );
                // End of the run: yes, always here.
                emit_library_changed(&handle, true);
            }
            Err(e) => {
                log::warn!("[sync] {name} failed: {e}");
                // A dropped connection isn't a failure worth a banner: the
                // phone went to sleep, switched networks, or the app closed
                // on the other side. Automatic sync retries on its own. It's
                // still reported when a person requested the sync and is
                // watching.
                let cut = is_disconnect(&e);
                // A broken pairing is already on screen, on the device's row:
                // a banner on every automatic attempt would add nothing.
                let quiet = e.is::<PairingBroken>() || e.is::<PeerBusy>();
                let error = if cut {
                    if auto {
                        None
                    } else {
                        Some("connection lost".to_string())
                    }
                } else if quiet && auto {
                    None
                } else {
                    Some(e.to_string())
                };
                let _ = handle.emit(
                    "sync-done",
                    SyncDoneEvent {
                        uid: uid.clone(),
                        name,
                        received: 0,
                        sent: 0,
                        failed: 0,
                        bytes: 0,
                        organized: 0,
                        auto,
                        error,
                    },
                );
            }
        }
        return;
    }
}

/// Waits between attempts against a busy device. It grows: a first sync of
/// a whole library on the other end takes minutes.
const BUSY_RETRY_MIN: Duration = Duration::from_secs(3);
const BUSY_RETRY_MAX: Duration = Duration::from_secs(30);
/// Past this it stops insisting; the next trigger (a change, the periodic
/// pass, the device showing up again) tries again.
const BUSY_GIVE_UP: Duration = Duration::from_secs(20 * 60);

/// Up to two seconds of noise. Two devices that call each other at the same
/// instant both get `Busy`; retrying with the exact same wait, they'd keep
/// colliding.
fn jitter() -> Duration {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    Duration::from_millis(u64::from(nanos % 2000))
}

/// A full run against an already paired device.
///
/// Opening the session is the only part still handled by the app (address,
/// keys, known devices); from there on the engine takes over, the same code
/// exercised by the integrity test suite (see engine.rs).
fn sync_inner(handle: &AppHandle, uid: &str) -> Result<engine::SyncResult> {
    let mut sess = open_session(handle, uid)?;
    engine::sync(&crate::AppHost(handle), &mut sess, uid)
}

fn platform_of(handle: &AppHandle, uid: &str) -> String {
    handle
        .state::<AppState>()
        .peers
        .list()
        .into_iter()
        .find(|p| p.uid == uid)
        .map(|p| p.platform)
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Shared
// ---------------------------------------------------------------------------

/// Proof that the channel stayed alive: each side tells the other how much
/// library it has. It's the first thing seen working end to end without
/// having moved a single byte of audio.
fn exchange_hello(handle: &AppHandle, sess: &mut Session, uid: &str, name: &str) -> Result<()> {
    let (my_uid, my_name) = me(handle)?;
    let (tracks, playlists) = library_counts(handle);
    sess.send(&Msg::Hello {
        uid: my_uid,
        name: my_name,
        platform: platform(),
        tracks,
        playlists,
        clock_ms: db::now_ms(),
    })?;
    match sess.recv()? {
        Msg::Hello {
            tracks,
            playlists,
            clock_ms,
            ..
        } => {
            report_hello(handle, uid, name, tracks, playlists, clock_ms);
            Ok(())
        }
        other => Err(anyhow!("expected Hello, got {other:?}")),
    }
}

fn report_hello(
    handle: &AppHandle,
    uid: &str,
    name: &str,
    tracks: i64,
    playlists: i64,
    their_clock: i64,
) {
    let skew = their_clock - db::now_ms();
    if skew.abs() > 5 * 60 * 1000 {
        log::warn!("[pair] clock of {name} is off by {skew} ms - last-write-wins may pick the wrong side");
    }
    {
        let state = handle.state::<AppState>();
        // The guard goes into its own variable: as an `if let` binding it
        // would be a temporary that outlives `state`, and it wouldn't compile.
        let db = state.db.lock();
        if let Ok(conn) = db {
            core_pair::touch_device(&conn, uid, name);
        }
    }
    let _ = handle.emit(
        "peer-hello",
        PeerHelloEvent {
            uid: uid.to_string(),
            name: name.to_string(),
            tracks,
            playlists,
            clock_skew_ms: skew,
        },
    );
}

fn emit_done(handle: &AppHandle, uid: &str, name: &str, ok: bool, error: Option<&str>) {
    let _ = handle.emit(
        "pairing-done",
        PairingDoneEvent {
            uid: uid.to_string(),
            name: name.to_string(),
            ok,
            error: error.map(|s| s.to_string()),
        },
    );
}

/// A different key for a known uid could be a reinstall on the other side —
/// or someone impersonating it. It doesn't resolve itself: the device is
/// marked as broken (the UI offers to forget it) and it's logged, once.
/// Trust is never restored on its own: forgetting and pairing again is the
/// person's call.
fn key_mismatch(handle: &AppHandle, uid: &str, name: &str) {
    if !mark_broken(handle, uid, KEY_CHANGED) {
        return;
    }
    let state = handle.state::<AppState>();
    // The guard goes into its own variable: as an `if let` binding it
    // would be a temporary that outlives `state`, and it wouldn't compile.
    let db = state.db.lock();
    if let Ok(conn) = db {
        core_pair::log_key_mismatch(&conn, uid, name);
    }
}

/// Opening a session with `uid` failed. If it was the version exchange (2.8),
/// the device is marked "update needed" naming who, and the error becomes a
/// `PairingBroken`: retrying can't fix it until someone updates, and the
/// watcher and autosync already know to stop on that.
fn session_failed(handle: &AppHandle, uid: &str, e: anyhow::Error) -> anyhow::Error {
    let Some(v) = e.downcast_ref::<sway_core::wire::VersionMismatch>() else { return e };
    let note = crate::discovery::update_note(
        &peer_name(handle, uid),
        &platform_of(handle, uid),
        v.peer_is_older,
        &v.their_app,
    );
    if handle.state::<AppState>().peers.set_outdated(uid, &note) {
        log::warn!("[pair] {uid}: {v}");
        let _ = handle.emit("peers-changed", ());
    }
    PairingBroken(note).into()
}

/// Marks the pairing with `uid` as broken (Fase 2.3). `true` if it's news.
fn mark_broken(handle: &AppHandle, uid: &str, reason: &str) -> bool {
    let state = handle.state::<AppState>();
    if !state.peers.set_invalid(uid, reason) {
        return false;
    }
    log::warn!("[pair] pairing with {uid} is broken: {reason}");
    let _ = handle.emit("peers-changed", ());
    true
}

/// The pairing just worked end to end: whatever was marked broken isn't.
fn pairing_ok(handle: &AppHandle, uid: &str) {
    if handle.state::<AppState>().peers.clear_invalid(uid) {
        let _ = handle.emit("peers-changed", ());
    }
}

/// Same, for a connection a person asked for: they're looking at a dialog.
fn warn_key_mismatch(handle: &AppHandle, uid: &str, name: &str) {
    key_mismatch(handle, uid, name);
    emit_done(
        handle,
        uid,
        name,
        false,
        Some("this device's key changed - unlink it and pair again if that was you"),
    );
}

fn forget_device(handle: &AppHandle, uid: &str) -> Result<()> {
    let state = handle.state::<AppState>();
    let pruned = {
        let conn = state.db.lock().map_err(|_| anyhow!("db lock"))?;
        core_pair::forget_device(&conn, uid)?;
        // Playlists only that device had, and this one doesn't hold, go too.
        sway_core::scope::prune_orphans(&conn)?
    };
    if pruned > 0 {
        emit_library_changed(handle, true);
    }
    state.peers.clear_invalid(uid);
    // A watcher napping against a broken device would otherwise sleep its
    // hour before noticing the device is gone.
    state.watchers.wake(uid);
    Ok(())
}

/// Removes a device from the trusted list and **notifies it**.
///
/// Pairing is stored on both sides. Without the notification, unpairing here
/// would leave the other side showing "Paired" forever, and no Refresh was
/// going to fix it: the device list has nothing to do with what mDNS sees.
/// The notification is best-effort — if the other side is off, it finds out
/// only the next time it tries to connect and gets `NotPaired`.
///
/// Also works on a server that already dropped us (its token changed): it's
/// no longer paired, but it's still on the list showing "Needs token", and
/// this is how it leaves.
pub fn unpair(handle: &AppHandle, uid: &str) -> Result<()> {
    let addr = peer_addr(handle, uid).ok();
    let paired = {
        let state = handle.state::<AppState>();
        let db = state.db.lock();
        match db {
            Ok(conn) => conn
                .query_row("SELECT 1 FROM devices WHERE uid = ?1", [uid], |_| Ok(()))
                .is_ok(),
            Err(_) => false,
        }
    };
    // A server also leaves the on-screen list. A LAN peer gets re-announced
    // by mDNS within seconds; nobody announces this one, so it would stay
    // there forever with a "Pair" button that can't work — that button
    // drives the local-network flow, with no token, and a server with no
    // token says no.
    let server = platform_of(handle, uid) == core_pair::PLATFORM_SERVER;
    forget_device(handle, uid)?;
    if server {
        handle.state::<AppState>().peers.forget(uid);
    }
    // Nothing to tell a device that no longer has us.
    if let Some(addr) = addr.filter(|_| paired) {
        let handle = handle.clone();
        let uid = uid.to_string();
        std::thread::spawn(move || {
            if let Err(e) = notify_unpair(&handle, addr) {
                log::debug!("[pair] could not notify unpair to {uid}: {e}");
            }
        });
    }
    Ok(())
}

fn notify_unpair(handle: &AppHandle, addr: SocketAddr) -> Result<()> {
    let stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)?;
    stream.set_read_timeout(Some(CONNECT_TIMEOUT))?;
    stream.set_write_timeout(Some(CONNECT_TIMEOUT))?;
    let private = private_key(handle)?;
    let mut sess = Session::connect(stream, &private)?;
    let (my_uid, _) = me(handle)?;
    sess.send(&Msg::Unpair { uid: my_uid })
}

#[cfg(test)]
mod tests {
    use super::{split_addr, split_host_port};

    #[test]
    fn pasting_the_server_url_works() {
        // It's what people have at hand and what anyone would paste.
        assert_eq!(
            split_host_port("https://sway.example.com/", 7420),
            ("sway.example.com".into(), 7420)
        );
        assert_eq!(
            split_host_port("http://sway.example.com:9000/something?x=1", 7420),
            ("sway.example.com".into(), 9000)
        );
        assert_eq!(
            split_host_port("  sway.example.com  ", 7420),
            ("sway.example.com".into(), 7420)
        );
    }

    #[test]
    fn the_written_port_wins_over_the_field() {
        // More specific, and it's the one the person just read somewhere.
        assert_eq!(
            split_host_port("192.168.0.10:9999", 7420),
            ("192.168.0.10".into(), 9999)
        );
    }

    #[test]
    fn an_ipv6_is_not_confused_with_a_port() {
        // The `:` in an IPv6 don't separate anything: no brackets, no port.
        assert_eq!(split_host_port("fd00::1", 7420), ("fd00::1".into(), 7420));
        assert_eq!(
            split_host_port("[fd00::1]:9000", 7420),
            ("fd00::1".into(), 9000)
        );
        assert_eq!(split_host_port("[fd00::1]", 7420), ("fd00::1".into(), 7420));
    }

    #[test]
    fn a_non_numeric_port_does_not_take_the_host_down_with_it() {
        // Better to try with the field's port than to fail over a typo.
        assert_eq!(
            split_host_port("home.example:port", 7420),
            ("home.example".into(), 7420)
        );
    }

    #[test]
    fn the_stored_address_splits_into_host_and_port() {
        assert_eq!(split_addr("home.example:7420"), Some(("home.example", 7420)));
        assert_eq!(split_addr("192.168.0.10:7420"), Some(("192.168.0.10", 7420)));
        // IPv6: the host goes in brackets precisely because it contains `:`.
        assert_eq!(split_addr("[fd00::1]:7420"), Some(("fd00::1", 7420)));
    }

    #[test]
    fn an_address_without_a_port_does_not_invent_one() {
        // Better to end up without the peer than to probe a port nobody chose.
        assert_eq!(split_addr("home.example"), None);
        assert_eq!(split_addr("home.example:"), None);
        assert_eq!(split_addr("home.example:port"), None);
        assert_eq!(split_addr(""), None);
    }
}
