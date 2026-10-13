//! Discovery of Trak devices on the local network (mDNS / Bonjour).
//!
//! Phase 5.1: discovery ONLY. Publishes this device and listens for others;
//! it doesn't connect, doesn't transfer, and doesn't trust anyone. Encrypted
//! pairing is 5.2.
//!
//! What's published in the TXT record is the minimum needed to list a device
//! and later open a connection: who it is (stable uid), what to call it,
//! what platform it runs on, and what protocol version it speaks. Nothing
//! about the library and nothing sensitive: anyone on the LAN can see these
//! records.
//!
//! **Android:** without `MulticastLock` NO packet arrives, and without error
//! — Wi-Fi drops multicast to save battery. This is handled in
//! `MainActivity.kt`. If someday "no device shows up" on the phone but does
//! on the PC, start there.

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};

pub const SERVICE_TYPE: &str = "_trak._tcp.local.";

/// What to tell the user when one of the two has to update (2.8), naming
/// who. `their_app` is the other side's app version, when known.
pub fn update_note(name: &str, platform: &str, peer_is_older: bool, their_app: &str) -> String {
    let theirs = if their_app.is_empty() {
        String::new()
    } else {
        format!(" ({their_app})")
    };
    if !peer_is_older {
        format!("{name} runs a newer version of Trak{theirs} — update this app")
    } else if platform == trak_core::pairing::PLATFORM_SERVER {
        format!("{name} runs an older Trak Server{theirs} — update trak-server")
    } else {
        format!("{name} runs an older version of Trak{theirs} — update it there")
    }
}

/// The protocol range a peer announces, checked against ours. Before 2.8
/// there was only `proto` ("1"), and that's below every minimum since.
fn announced_update(name: &str, platform: &str, proto: &str, min: &str) -> Option<String> {
    let theirs: u32 = proto.parse().unwrap_or(0);
    let their_min: u32 = min.parse().unwrap_or(theirs);
    match trak_core::wire::compat(theirs, their_min) {
        trak_core::wire::Compat::Ok => None,
        c => Some(update_note(name, platform, c == trak_core::wire::Compat::PeerOlder, "")),
    }
}


/// How often to check that peers are still reachable. A connect to the LAN
/// is cheap; this is what makes grey show up in seconds instead of waiting
/// for the mDNS TTL to expire.
const PROBE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1200);
/// How long after startup an absent paired device still shows as "checking"
/// instead of "offline". mDNS needs a moment to hear the others; painting
/// them grey and then green a few seconds later was the other half of
/// Fase 2.2.
const DISCOVERY_WINDOW: std::time::Duration = std::time::Duration::from_secs(6);
/// How many probes between re-querying the network (6 x 10 s = 1 minute).
const REBROWSE_EVERY: u32 = 6;

// Note: there's no time-based expiry on this side. mdns-sd handles that: it
// refreshes records before they expire and emits `ServiceRemoved` when one
// really does expire — which does NOT mean the device is gone, only that its
// announcements stopped arriving; who's available is decided by the TCP
// probe further below. A homegrown filter by "how long since I saw it" is a
// bug waiting to happen: `ServiceResolved` fires when something CHANGES, not
// on every refresh, so a peer that's present and stable stops emitting
// events and would disappear from the list while still being there.

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Peer {
    pub uid: String,
    pub name: String,
    pub platform: String,
    pub addrs: Vec<String>,
    pub port: u16,
    pub last_seen: i64,
    /// Already in `devices`: this device was paired with at some point.
    /// Resolved on listing, not on discovery — otherwise pairing wouldn't
    /// show up until the next startup.
    pub paired: bool,
    /// Reachable right now, per the TCP probe or a standing connection.
    /// Paired devices that aren't there get listed anyway, greyed out:
    /// they're still your devices even if the phone is off wifi.
    pub online: bool,
    /// Nobody has tried to reach it yet: just announced by mDNS, restored
    /// from the DB, or the app just started. Shown as "checking", neither
    /// green nor grey (Fase 2.2): a peer used to be born `online` straight
    /// from the mDNS announcement, and the UI said "connected" before any
    /// connection had been tried.
    pub checking: bool,
    /// The pairing no longer works and won't fix itself: the other side
    /// answered with a different key, or rejected ours (Fase 2.3). Carries
    /// the reason to show. Autosync and the watcher leave the device alone,
    /// and the UI offers to forget it.
    pub invalid: Option<String>,
    /// One of the two has to update before they can sync (Fase 2.8): the
    /// protocols are out of range. Says who, ready to show. From the mDNS
    /// announcement, or from a session that failed on the version exchange.
    /// Like `invalid`, autosync and the watcher leave the device alone.
    pub update: Option<String>,
}

pub struct Peers {
    by_uid: Mutex<HashMap<String, Peer>>,
    /// mDNS's removal event carries the fullname, not the uid.
    fullname_to_uid: Mutex<HashMap<String, String>>,
    /// Broken pairings, by uid. Apart from `by_uid` because a paired device
    /// that isn't on the network isn't in that map either, and it can still
    /// be the broken one.
    invalid: Mutex<HashMap<String, String>>,
    /// Version mismatches seen when opening a session, by uid: the message
    /// for `Peer::update`.
    outdated: Mutex<HashMap<String, String>>,
    born: std::time::Instant,
}

impl Default for Peers {
    fn default() -> Self {
        Peers {
            by_uid: Mutex::default(),
            fullname_to_uid: Mutex::default(),
            invalid: Mutex::default(),
            outdated: Mutex::default(),
            born: std::time::Instant::now(),
        }
    }
}

/// What happened when recording an mDNS announcement.
struct Seen {
    /// Something changed that the user would see differently: needs a
    /// re-render.
    changed: bool,
    /// Wasn't in the list, or was grey: it just showed up on the network and
    /// has to be probed now. Being announced isn't being reachable, so it's
    /// the probe that turns it green — and that transition is what triggers
    /// the catch-up sync, same as for a device that came back.
    first_time: bool,
}

impl Peers {
    /// Known peers, sorted by name. Being on this list isn't being
    /// available: that's what `online` says, decided by the TCP probe.
    /// Paired peers stay even after mDNS stops announcing them (see
    /// `mark_gone_by_fullname`); unknown ones do go away.
    pub fn list(&self) -> Vec<Peer> {
        let mut v: Vec<Peer> = self.by_uid.lock().unwrap().values().cloned().collect();
        v.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        v
    }

    /// What the UI sees: the ones on the network NOW, plus the already
    /// paired ones that aren't (off, without wifi, out of range).
    ///
    /// `paired` comes from the DB on every call, not from what's cached in
    /// discovery: pairing doesn't change anything mDNS announces, so a flag
    /// cached there would go stale until the peer re-announces.
    pub fn merged_list(&self, conn: &rusqlite::Connection) -> Vec<Peer> {
        let mut out = self.list();
        let known = paired_devices(conn);
        let invalid = self.invalid.lock().unwrap().clone();
        let outdated = self.outdated.lock().unwrap().clone();
        for p in out.iter_mut() {
            p.paired = known.iter().any(|(uid, _, _, _)| uid == &p.uid);
            p.invalid = invalid.get(&p.uid).cloned();
            // What a session found beats the announcement: it's newer.
            if let Some(note) = outdated.get(&p.uid) {
                p.update = Some(note.clone());
            }
        }
        // Not heard from yet. Right after startup that's "still looking",
        // not "off".
        let settling = self.settling();
        for (uid, name, platform, last_seen) in known {
            if out.iter().any(|p| p.uid == uid) {
                continue;
            }
            out.push(Peer {
                invalid: invalid.get(&uid).cloned(),
                update: outdated.get(&uid).cloned(),
                uid,
                name,
                platform,
                addrs: Vec::new(),
                port: 0,
                last_seen,
                paired: true,
                online: false,
                checking: settling,
            });
        }
        // First the ones on the network; then alphabetical.
        out.sort_by(|a, b| {
            b.online
                .cmp(&a.online)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        out
    }

    /// Records an announcement. What the probe learned is kept: an
    /// announcement says where the peer is, not whether it answers.
    fn upsert(&self, fullname: String, mut peer: Peer) -> Seen {
        self.fullname_to_uid
            .lock()
            .unwrap()
            .insert(fullname, peer.uid.clone());
        let mut map = self.by_uid.lock().unwrap();
        // "Changed" = something the user would see differently. A last_seen-
        // only refresh isn't worth emitting: mDNS re-queries often and would
        // make the list re-render all the time.
        let seen = match map.get(&peer.uid) {
            Some(old) => {
                peer.online = old.online;
                peer.checking = old.checking;
                Seen {
                    changed: old.name != peer.name
                        || old.addrs != peer.addrs
                        || old.port != peer.port,
                    // Announced again after being grey: probe it now. For
                    // sync purposes that's the same as appearing for the
                    // first time — it could have been off for hours.
                    first_time: !old.online,
                }
            }
            None => {
                peer.online = false;
                peer.checking = true;
                Seen { changed: true, first_time: true }
            }
        };
        map.insert(peer.uid.clone(), peer);
        seen
    }

    /// Still inside the startup window: absent paired devices are "checking".
    fn settling(&self) -> bool {
        self.born.elapsed() < DISCOVERY_WINDOW
    }

    /// Nothing left to hear about: the startup window passed and every
    /// announced peer was probed at least once.
    fn settled(&self) -> bool {
        !self.settling() && !self.by_uid.lock().unwrap().values().any(|p| p.checking)
    }

    /// Records that the pairing with `uid` is broken. `true` if it wasn't
    /// already: the caller logs it once, not on every retry.
    pub fn set_invalid(&self, uid: &str, reason: &str) -> bool {
        self.invalid
            .lock()
            .unwrap()
            .insert(uid.to_string(), reason.to_string())
            .is_none()
    }

    /// Records that `uid` speaks a protocol out of range (2.8). `true` if
    /// it's news.
    pub fn set_outdated(&self, uid: &str, note: &str) -> bool {
        self.outdated
            .lock()
            .unwrap()
            .insert(uid.to_string(), note.to_string())
            .as_deref()
            != Some(note)
    }

    /// Syncing with `uid` can't work until someone acts: the pairing is
    /// broken, or one side has to update. Retrying won't fix either.
    pub fn is_invalid(&self, uid: &str) -> bool {
        self.invalid.lock().unwrap().contains_key(uid)
            || self.outdated.lock().unwrap().contains_key(uid)
            || self
                .by_uid
                .lock()
                .unwrap()
                .get(uid)
                .is_some_and(|p| p.update.is_some())
    }

    /// Forgotten, paired again, or a session just worked: whatever was
    /// wrong no longer applies.
    pub fn clear_invalid(&self, uid: &str) -> bool {
        let broken = self.invalid.lock().unwrap().remove(uid).is_some();
        let outdated = self.outdated.lock().unwrap().remove(uid).is_some();
        broken || outdated
    }

    /// `(uid, ip, port)` for each peer with a known address, to probe.
    fn probe_targets(&self) -> Vec<(String, String, u16)> {
        self.by_uid
            .lock()
            .unwrap()
            .values()
            .filter_map(|p| p.addrs.first().map(|a| (p.uid.clone(), a.clone(), p.port)))
            .collect()
    }

    fn probe_target(&self, uid: &str) -> Option<(String, String, u16)> {
        self.probe_targets().into_iter().find(|(u, _, _)| u == uid)
    }

    /// Marks a peer unreachable after a failed connection attempt, without
    /// waiting for the next probe.
    pub fn mark_unreachable(&self, uid: &str) -> bool {
        self.set_online(uid, false)
    }

    /// Removes a manually added device from the list.
    ///
    /// Not needed for a LAN peer: mDNS re-announces it within seconds. A
    /// fixed-address one isn't announced by anyone, so if it's not removed
    /// here it stays on screen forever — unpaired, but with a "Pair" button
    /// that can't work: a server needs its token, and that button drives the
    /// local-network flow, which doesn't carry one.
    pub fn forget(&self, uid: &str) -> bool {
        self.clear_invalid(uid);
        self.by_uid.lock().unwrap().remove(uid).is_some()
    }

    /// A device with a fixed address, one nobody will discover (Phase 6.3).
    ///
    /// It enters the same list as the ones mDNS announces, and that's the
    /// whole point: from here down — the probe, the online state, the
    /// automatic sync when it comes back — there isn't a single branch that
    /// asks where it came from. The difference is how the app learned about
    /// it, not what it is.
    ///
    /// It can't be removed via `ServiceRemoved`: that path looks up by mDNS
    /// fullname and this peer doesn't have one.
    pub fn add_manual(&self, uid: &str, name: &str, platform: &str, host: &str, port: u16) {
        let mut map = self.by_uid.lock().unwrap();
        let entry = map.entry(uid.to_string()).or_insert_with(|| Peer {
            uid: uid.to_string(),
            name: name.to_string(),
            platform: platform.to_string(),
            addrs: Vec::new(),
            port,
            last_seen: crate::db::now_ms(),
            paired: true,
            // Not known yet: the probe decides it. Being born `true` would
            // show a server as online when the connection wasn't even tried.
            online: false,
            checking: true,
            invalid: None,
            update: None,
        });
        entry.addrs = vec![host.to_string()];
        entry.port = port;
        entry.name = name.to_string();
        entry.platform = platform.to_string();
    }

    /// Returns `true` if the state changed (i.e. if there's something to
    /// show). Any answer ends "checking", even a "no".
    pub(crate) fn set_online(&self, uid: &str, online: bool) -> bool {
        let mut map = self.by_uid.lock().unwrap();
        match map.get_mut(uid) {
            Some(p) if p.online != online || p.checking => {
                p.checking = false;
                p.online = online;
                if online {
                    p.last_seen = crate::db::now_ms();
                }
                true
            }
            _ => false,
        }
    }

    fn remove_by_fullname(&self, fullname: &str) -> bool {
        let uid = self.fullname_to_uid.lock().unwrap().remove(fullname);
        match uid {
            Some(uid) => self.by_uid.lock().unwrap().remove(&uid).is_some(),
            None => false,
        }
    }

    fn uid_of(&self, fullname: &str) -> Option<String> {
        self.fullname_to_uid.lock().unwrap().get(fullname).cloned()
    }

    /// No longer announcing turns the peer grey, but does **not** remove it
    /// from the list: that way the TCP probe keeps trying it and can bring
    /// it back to green.
    ///
    /// Removing it was a dead end. `probe_targets` walks this same map, so a
    /// removed peer would never be probed again: the honest mechanism for
    /// knowing whether it's still there couldn't contradict the one that had
    /// gotten it wrong. And getting it wrong is common — SRV/A records live
    /// 120 s and are refreshed via multicast, which gets lost for all sorts
    /// of reasons that have nothing to do with the device being gone:
    /// Wi-Fi radio power saving, Android's `MulticastLock`, a band switch, a
    /// router that filters it. In all those cases the TCP connection stays
    /// open.
    ///
    /// `fullname_to_uid` is kept so a later removal of the same peer can
    /// still be resolved.
    fn mark_gone_by_fullname(&self, fullname: &str) -> bool {
        match self.uid_of(fullname) {
            Some(uid) => self.set_online(&uid, false),
            None => false,
        }
    }
}

/// Starts announcing and browsing. The returned `ServiceDaemon` has to be
/// kept alive: dropping it stops publishing the service.
pub fn start(
    handle: AppHandle,
    uid: &str,
    name: &str,
    port: u16,
) -> Result<ServiceDaemon, Box<dyn std::error::Error>> {
    let daemon = ServiceDaemon::new()?;

    // The hostname has to be unique on the network, so it's derived from the
    // uid and not the name the user set: two devices named "PC" would
    // clobber each other's record.
    let short = uid.split('-').next().unwrap_or(uid).to_string();
    let props: HashMap<String, String> = HashMap::from([
        ("uid".to_string(), uid.to_string()),
        ("name".to_string(), name.to_string()),
        ("platform".to_string(), platform_name()),
        ("proto".to_string(), trak_core::wire::PROTOCOL.to_string()),
        ("minproto".to_string(), trak_core::wire::MIN_PROTOCOL.to_string()),
    ]);
    let info = ServiceInfo::new(
        SERVICE_TYPE,
        &short,
        &format!("trak-{short}.local."),
        "",
        port,
        props,
    )?
    // The library follows IP changes on its own: on the phone the address
    // changes when switching from Wi-Fi to mobile data and back.
    .enable_addr_auto();
    daemon.register(info)?;
    log::info!("[mdns] published {name} ({uid}) on port {port}");

    spawn_browse_loop(handle, daemon.browse(SERVICE_TYPE)?, uid.to_string());
    Ok(daemon)
}

/// Asks who's on the network again, right now.
///
/// A manual trigger is needed because `mdns-sd` **doubles the wait between
/// queries** on every round (1 s, 2 s, 4 s… capped at one hour, RFC 6762
/// §5.2). That's correct behavior to avoid flooding the network, but it
/// means that if the first multicast packets were lost — common on Wi-Fi —
/// the peer only shows up several minutes later. A fresh `browse` sends the
/// query immediately and resets that count.
///
/// The previous listener gets replaced (`service_queriers` is indexed by
/// service type), so its thread ends on its own: they don't pile up.
pub fn refresh(handle: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let (receiver, uid) = {
        let state = handle.state::<crate::AppState>();
        let guard = state.mdns.lock().map_err(|_| "mdns lock")?;
        let daemon = guard.as_ref().ok_or("discovery is not active")?;
        let receiver = daemon.browse(SERVICE_TYPE)?;
        let conn = state.db.lock().map_err(|_| "db lock")?;
        (receiver, crate::db::this_device_uid(&conn)?)
    };
    spawn_browse_loop(handle.clone(), receiver, uid);
    Ok(())
}

fn spawn_browse_loop(handle: AppHandle, receiver: mdns_sd::Receiver<ServiceEvent>, me: String) {
    std::thread::spawn(move || {
        for event in receiver {
            match event {
                ServiceEvent::ServiceResolved(info) => {
                    let txt = &info.txt_properties;
                    let get = |k: &str| txt.get_property_val_str(k).unwrap_or("").to_string();
                    let peer_uid = get("uid");
                    // We see ourselves via multicast loopback.
                    if peer_uid.is_empty() || peer_uid == me {
                        continue;
                    }
                    // Link-local addresses (169.254.x from virtual adapters,
                    // fe80::) go last: they're real addresses but useless
                    // for connecting, and on this machine they were winning
                    // by alphabetical order. The first in the list is what
                    // the UI shows and what 5.2 will use.
                    let mut addrs: Vec<(bool, String)> = info
                        .addresses
                        .iter()
                        .filter(|a| !a.is_loopback())
                        .map(|a| (is_link_local(a), a.to_string()))
                        .collect();
                    addrs.sort();
                    let addrs: Vec<String> = addrs.into_iter().map(|(_, a)| a).collect();
                    let name = {
                        let n = get("name");
                        if n.is_empty() { peer_uid.clone() } else { n }
                    };
                    let platform = get("platform");
                    let update =
                        announced_update(&name, &platform, &get("proto"), &get("minproto"));
                    let peer = Peer {
                        uid: peer_uid.clone(),
                        name,
                        platform,
                        addrs,
                        port: info.port,
                        last_seen: crate::db::now_ms(),
                        // Resolved against the DB by `merged_list` on every
                        // read; caching them here would leave them stale.
                        paired: false,
                        // Decided by `upsert`: an announcement isn't a
                        // connection.
                        online: false,
                        checking: true,
                        invalid: None,
                        update,
                    };
                    let state = handle.state::<crate::AppState>();
                    let seen = state.peers.upsert(info.fullname.clone(), peer);
                    if seen.changed {
                        log::info!("[mdns] peer visible: {}", info.fullname);
                        let _ = handle.emit("peers-changed", ());
                    }
                    // Showed up on the network: probe it now, not in ten
                    // seconds. Turning green is what triggers the catch-up
                    // (see `apply_probe`); without it you'd have to wait for
                    // something to change here or for the 10-minute safety
                    // net, and in practice people ended up hitting "sync" by
                    // hand when opening the app.
                    if seen.first_time {
                        let handle = handle.clone();
                        std::thread::spawn(move || probe_uid(&handle, &peer_uid));
                    }
                }
                ServiceEvent::ServiceRemoved(_, fullname) => {
                    let state = handle.state::<crate::AppState>();
                    // For our own devices, mDNS has an opinion and the probe
                    // decides: they go grey and keep being probed. For an
                    // unknown one there's nothing to salvage — no one is
                    // going to sync with it — and leaving it grey forever
                    // would just be clutter in the list.
                    let ours = match state.peers.uid_of(&fullname) {
                        Some(uid) => is_paired(&handle, &uid),
                        None => false,
                    };
                    if ours {
                        if state.peers.mark_gone_by_fullname(&fullname) {
                            log::info!("[mdns] {fullname} stopped announcing: greyed out until the probe says otherwise");
                            let _ = handle.emit("peers-changed", ());
                        }
                    } else if state.peers.remove_by_fullname(&fullname) {
                        log::info!("[mdns] peer gone: {fullname}");
                        let _ = handle.emit("peers-changed", ());
                    }
                }
                _ => {}
            }
        }
        log::debug!("[mdns] listener replaced by a new one");
    });
}

/// Reachability probe: opens a TCP connection to each peer's sync port and
/// closes it right away.
///
/// Needed because mDNS doesn't tell you whether someone is STILL there, and
/// it fails in both directions. It stays too long: PTR/TXT records have a
/// TTL of 4500 s (75 minutes) and `mdns-sd` 0.20 doesn't expose a way to
/// lower it, so a device that leaves without notice stays in the cache for
/// over an hour. And it leaves too soon: SRV/A/AAAA records live 120 s (RFC
/// 6762 §10) and are refreshed via multicast, which just gets lost on its
/// own. Worse: "announced" and "reachable" aren't the same thing — the phone
/// can stay in the cache with the app closed, and there Ping eats the
/// timeout. A connect is the only honest answer to "can I sync with this
/// right now?".
///
/// On the other side this hits the accept loop and disconnects without
/// sending anything: `serve` recognizes it as a probe from the EOF and
/// doesn't report it as an error.
pub fn spawn_prober(handle: AppHandle) {
    std::thread::spawn(move || {
        // Right away, not after the first interval: the restored server
        // stays "checking" until someone tries it.
        probe_once(&handle);
        // Absent paired devices stop being "checking" when the window
        // closes; nothing else would repaint them.
        let born = handle.state::<crate::AppState>().peers.born;
        std::thread::sleep(DISCOVERY_WINDOW.saturating_sub(born.elapsed()));
        let _ = handle.emit("peers-changed", ());
        let mut tick: u32 = 0;
        loop {
            std::thread::sleep(PROBE_INTERVAL);
            tick += 1;

            // Re-query on our own every so often: without this, mDNS's
            // backoff stops querying for up to an hour, and a device that
            // powers on afterward stays invisible the whole time.
            if tick % REBROWSE_EVERY == 0 {
                if let Err(e) = refresh(&handle) {
                    log::debug!("[mdns] re-browse failed: {e}");
                }
            }

            probe_once(&handle);
        }
    });
}

/// One round of probing. Emits `peers-changed` only if some state changed.
pub fn probe_once(handle: &AppHandle) {
    probe_round(handle, false);
}

/// Same, including devices with a standing connection. For the Refresh
/// button: the person asked for the truth now, not for what the connection
/// last said.
pub fn probe_all(handle: &AppHandle) {
    probe_round(handle, true);
}

fn probe_round(handle: &AppHandle, all: bool) {
    let state = handle.state::<crate::AppState>();
    let results = state
        .peers
        .probe_targets()
        .into_iter()
        // With a standing connection open (see `watch.rs`) there's nothing
        // to probe: that connection already proves the device is there, and
        // better than a connect, because it drops the moment it stops being
        // there. Probing it anyway would mean opening a socket every ten
        // seconds against a device we already have on the other end of the
        // phone.
        .filter(|(uid, _, _)| all || !state.watchers.is_live(uid))
        .map(|(uid, ip, port)| {
            // In parallel: one unreachable device costs its timeout, not the
            // sum of all of them — the Refresh button waits for this.
            std::thread::spawn(move || {
                let ok = reachable(&ip, port);
                (uid, ok)
            })
        })
        .collect::<Vec<_>>()
        .into_iter()
        .filter_map(|t| t.join().ok())
        .collect();
    apply_probe(handle, results);
}

/// Probes a single peer, now. For one that was just announced: waiting for
/// the next round left it "checking" for up to ten seconds.
fn probe_uid(handle: &AppHandle, uid: &str) {
    let state = handle.state::<crate::AppState>();
    if state.watchers.is_live(uid) {
        return;
    }
    // Announced without a usable address: nothing to try, and that's an
    // answer too — left "checking", it would hold up `wait_settled` forever.
    let ok = match state.peers.probe_target(uid) {
        Some((_, ip, port)) => reachable(&ip, port),
        None => false,
    };
    apply_probe(handle, vec![(uid.to_string(), ok)]);
}

fn reachable(ip: &str, port: u16) -> bool {
    match resolve(ip, port) {
        Some(addr) => std::net::TcpStream::connect_timeout(&addr, PROBE_TIMEOUT).is_ok(),
        None => false,
    }
}

/// Records what the probe found, repaints, and catches up with whoever just
/// became reachable.
fn apply_probe(handle: &AppHandle, results: Vec<(String, bool)>) {
    let state = handle.state::<crate::AppState>();
    let mut changed = false;
    let mut came_online = Vec::new();
    for (uid, ok) in results {
        let was_online = state.peers.list().iter().any(|p| p.uid == uid && p.online);
        if state.peers.set_online(&uid, ok) {
            changed = true;
            if ok && !was_online {
                came_online.push(uid);
            }
        }
    }
    if changed {
        let _ = handle.emit("peers-changed", ());
    }
    // Became available again: catch up without waiting for something to
    // change. The phone might have been off wifi all afternoon. (Whether
    // it's paired is filtered by `peer_came_online`.)
    for uid in came_online {
        // And if there's a watcher waiting its turn against this device, let
        // it stop waiting: this is the only notice that the server came
        // back that will ever arrive, because the server can't give any.
        state.watchers.wake(&uid);
        crate::autosync::peer_came_online(handle, &uid);
    }
}

/// Waits (a few seconds at most) until discovery has had its say: the
/// startup window passed and every announced peer was probed once.
///
/// For whoever needs to know which LAN devices are there before deciding
/// something — the server catch-up, which has to go after the LAN (Fase
/// 2.4). Asking at t=0 found no LAN peer yet and started pulling from the
/// server while the PC was about to show up.
pub fn wait_settled(handle: &AppHandle) {
    const MAX: std::time::Duration = std::time::Duration::from_secs(12);
    const POLL: std::time::Duration = std::time::Duration::from_millis(250);
    let until = std::time::Instant::now() + MAX;
    let state = handle.state::<crate::AppState>();
    while !state.peers.settled() && std::time::Instant::now() < until {
        std::thread::sleep(POLL);
    }
}

/// Address to attempt the connection at.
///
/// Parsing alone isn't enough: mDNS announces IPs, but a manually added
/// server is almost always a name — a home doesn't have a fixed IP, and what
/// gets entered is the router's DDNS. `parse::<SocketAddr>` fails on a name,
/// and the peer would stay grey forever with no visible error.
pub fn resolve(host: &str, port: u16) -> Option<std::net::SocketAddr> {
    use std::net::ToSocketAddrs;
    (host, port).to_socket_addrs().ok()?.next()
}

/// Is it paired? Short query in its own scope: run by the mDNS thread, which
/// has no reason to hold the DB lock while a sync is running.
fn is_paired(handle: &AppHandle, uid: &str) -> bool {
    let state = handle.state::<crate::AppState>();
    let Ok(conn) = state.db.lock() else {
        return false;
    };
    conn.query_row("SELECT 1 FROM devices WHERE uid = ?1", [uid], |_| Ok(()))
        .is_ok()
}

/// `(uid, name, platform, last_seen)` of the already paired devices.
fn paired_devices(conn: &rusqlite::Connection) -> Vec<(String, String, String, i64)> {
    let mut stmt = match conn
        .prepare("SELECT uid, name, platform, COALESCE(last_seen, 0) FROM devices")
    {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)));
    match rows {
        Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
        Err(_) => Vec::new(),
    }
}

/// Self-assigned addresses when there's no DHCP (`169.254.0.0/16`,
/// `fe80::/10`). They show up on virtual adapters (WSL, VirtualBox, Hyper-V)
/// and aren't useful for reaching the peer.
fn is_link_local(ip: &mdns_sd::ScopedIp) -> bool {
    match ip.to_ip_addr() {
        std::net::IpAddr::V4(v4) => v4.is_link_local(),
        std::net::IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) == 0xfe80,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(peers: &Peers, uid: &str, name: &str) -> Seen {
        peers.upsert(
            format!("{uid}._trak._tcp.local."),
            Peer {
                uid: uid.into(),
                name: name.into(),
                platform: "windows".into(),
                addrs: vec!["192.168.0.5".into()],
                port: 1234,
                last_seen: crate::db::now_ms(),
                paired: false,
                online: true,
                checking: false,
                invalid: None,
                update: None,
            },
        )
    }

    /// Announced and then answered the probe.
    fn reachable_peer(peers: &Peers, uid: &str, name: &str) {
        seen(peers, uid, name);
        peers.set_online(uid, true);
    }

    fn db_with_device(uid: &str, name: &str) -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::init_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO devices (uid, name, platform, paired_at, last_seen)
             VALUES (?1, ?2, 'android', 1, 2)",
            rusqlite::params![uid, name],
        )
        .unwrap();
        conn
    }

    /// The `paired` flag has to come from the DB on every read. When it was
    /// cached on discovery, pairing wouldn't show up until the app
    /// restarted: mDNS doesn't re-announce anything when a device is saved.
    #[test]
    fn pairing_shows_up_without_a_new_mdns_announcement() {
        let peers = Peers::default();
        reachable_peer(&peers, "peer-1", "Phone");
        let empty = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::init_schema(&empty).unwrap();
        assert!(!peers.merged_list(&empty)[0].paired);

        // Same discovery state, but now paired.
        let paired = db_with_device("peer-1", "Phone");
        let list = peers.merged_list(&paired);
        assert_eq!(list.len(), 1);
        assert!(list[0].paired);
        assert!(list[0].online);
    }

    #[test]
    fn paired_devices_stay_listed_while_offline() {
        let peers = Peers::default();
        let conn = db_with_device("saved", "Living Room PC");
        let list = peers.merged_list(&conn);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "Living Room PC");
        assert!(list[0].paired);
        assert!(!list[0].online);
        assert!(list[0].addrs.is_empty());
    }

    /// A peer that's present and stable stops generating `ServiceResolved`
    /// events (they only arrive when something changes), so its `last_seen`
    /// goes stale. Filtering by that would make it "disappear" from the
    /// network while still there: whether it's still present is decided by
    /// mdns-sd via `ServiceRemoved`.
    #[test]
    fn a_quiet_peer_stays_online() {
        let peers = Peers::default();
        reachable_peer(&peers, "peer-1", "Phone");
        {
            let mut map = peers.by_uid.lock().unwrap();
            map.get_mut("peer-1").unwrap().last_seen = crate::db::now_ms() - 60 * 60 * 1000;
        }
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::init_schema(&conn).unwrap();
        let list = peers.merged_list(&conn);
        assert_eq!(list.len(), 1);
        assert!(list[0].online);
    }

    /// mDNS says "it was announced", not "it's still there": with a 75-
    /// minute TTL, a peer that left is still in the cache. The TCP probe is
    /// what decides whether it's online, and a failed connection attempt has
    /// to be reflected right away — staying "connected" right after a
    /// timeout is the worst possible combination.
    #[test]
    fn a_failed_connection_marks_the_peer_offline() {
        let peers = Peers::default();
        reachable_peer(&peers, "peer-1", "Phone");
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::init_schema(&conn).unwrap();
        assert!(peers.merged_list(&conn)[0].online);

        assert!(peers.mark_unreachable("peer-1"));
        assert!(!peers.merged_list(&conn)[0].online);
        // Second time doesn't change anything: no reason to re-emit the event.
        assert!(!peers.mark_unreachable("peer-1"));

        // And it comes back on its own once it responds again.
        assert!(peers.set_online("peer-1", true));
        assert!(peers.merged_list(&conn)[0].online);
    }

    /// An announcement isn't a connection (Fase 2.2): the peer is born
    /// "checking", and it's the probe that turns it green or grey.
    #[test]
    fn an_announced_peer_is_checking_until_probed() {
        let peers = Peers::default();
        seen(&peers, "peer-1", "Phone");
        let p = &peers.list()[0];
        assert!(!p.online, "announced is not reachable");
        assert!(p.checking);

        // A "no" from the probe also ends the checking, and is visible.
        assert!(peers.set_online("peer-1", false));
        let p = &peers.list()[0];
        assert!(!p.online && !p.checking);
        assert!(!peers.set_online("peer-1", false), "nothing new the second time");
    }

    /// Re-announcing doesn't undo what the probe learned.
    #[test]
    fn a_reannouncement_keeps_the_probed_state() {
        let peers = Peers::default();
        reachable_peer(&peers, "peer-1", "Phone");
        seen(&peers, "peer-1", "Phone");
        assert!(peers.list()[0].online);
        assert!(!peers.list()[0].checking);
    }

    /// Paired devices nobody has heard from yet are "checking" right after
    /// startup, not "offline".
    #[test]
    fn absent_paired_devices_are_checking_at_startup() {
        let peers = Peers::default();
        let conn = db_with_device("saved", "PC");
        assert!(peers.merged_list(&conn)[0].checking);
        assert!(!peers.settled());
    }

    /// A broken pairing shows up on the device, whether it's on the network
    /// or not, and forgetting it clears it.
    #[test]
    fn a_broken_pairing_is_listed_and_forgotten() {
        let peers = Peers::default();
        let conn = db_with_device("saved", "Server");
        assert!(peers.set_invalid("saved", "key changed"));
        assert!(!peers.set_invalid("saved", "key changed"), "only news once");
        assert_eq!(peers.merged_list(&conn)[0].invalid.as_deref(), Some("key changed"));
        peers.forget("saved");
        assert!(!peers.is_invalid("saved"));
        assert!(peers.merged_list(&conn)[0].invalid.is_none());
    }

    /// Showing up on the network means probing it; answering the probe is
    /// what triggers the catch-up (see `apply_probe`).
    #[test]
    fn a_peer_that_appears_for_the_first_time_is_a_catch_up_trigger() {
        let peers = Peers::default();
        assert!(seen(&peers, "peer-1", "Phone").first_time);
        peers.set_online("peer-1", true);

        // mDNS re-announces often and `refresh` re-asks every minute: that
        // can't count as an appearance or it would sync in a loop.
        let again = seen(&peers, "peer-1", "Phone");
        assert!(!again.first_time);
        assert!(!again.changed, "an identical re-announcement doesn't re-render");

        // Renaming it is visible, but it's still the same peer, present.
        let renamed = seen(&peers, "peer-1", "Guille's Phone");
        assert!(renamed.changed);
        assert!(!renamed.first_time);

        // Left the network and came back: now it does need to catch up again.
        assert!(peers.remove_by_fullname("peer-1._trak._tcp.local."));
        assert!(seen(&peers, "peer-1", "Phone").first_time);
    }

    #[test]
    fn removed_peers_disappear() {
        let peers = Peers::default();
        seen(&peers, "peer-1", "Phone");
        assert!(peers.remove_by_fullname("peer-1._trak._tcp.local."));
        assert!(peers.list().is_empty());
    }

    /// mDNS no longer announcing a paired device can't remove it from the
    /// list: `probe_targets` walks that same map, so removing it would leave
    /// it out of the TCP probe forever. In practice that meant minutes of
    /// "no device available" with the phone right there, reachable, waiting.
    #[test]
    fn a_peer_that_stops_announcing_goes_grey_but_stays_probed() {
        let peers = Peers::default();
        reachable_peer(&peers, "peer-1", "Phone");

        assert!(peers.mark_gone_by_fullname("peer-1._trak._tcp.local."));
        let list = peers.list();
        assert_eq!(list.len(), 1, "still in the list");
        assert!(!list[0].online, "but grey");
        assert_eq!(
            peers.probe_targets().len(),
            1,
            "and above all: it's still being probed"
        );

        // The probe finds it: goes back to green without mDNS saying anything.
        assert!(peers.set_online("peer-1", true));

        // A second removal in a row doesn't re-emit: it was already grey.
        assert!(peers.mark_gone_by_fullname("peer-1._trak._tcp.local."));
        assert!(!peers.mark_gone_by_fullname("peer-1._trak._tcp.local."));
    }

    /// A grey peer that re-announces has to be probed again: it could have
    /// been gone for hours, and answering is what triggers the catch-up.
    #[test]
    fn a_grey_peer_that_comes_back_is_a_catch_up_trigger() {
        let peers = Peers::default();
        reachable_peer(&peers, "peer-1", "Phone");
        peers.mark_gone_by_fullname("peer-1._trak._tcp.local.");

        let back = seen(&peers, "peer-1", "Phone");
        assert!(back.first_time, "was grey: could have been for hours");
        assert!(!peers.list()[0].online, "still grey until the probe answers");
        assert!(peers.set_online("peer-1", true), "and then the UI paints it green");

        // While green, an identical re-announcement is still nothing.
        let again = seen(&peers, "peer-1", "Phone");
        assert!(!again.first_time);
        assert!(!again.changed);
    }

    /// Those on the network go first; the rest, alphabetical.
    #[test]
    fn online_peers_sort_first() {
        let peers = Peers::default();
        reachable_peer(&peers, "zeta", "Zeta");
        let conn = db_with_device("alfa", "Alfa");
        let names: Vec<String> = peers.merged_list(&conn).into_iter().map(|p| p.name).collect();
        assert_eq!(names, vec!["Zeta", "Alfa"]);
    }
}

fn platform_name() -> String {
    if cfg!(target_os = "android") {
        "android".into()
    } else if cfg!(target_os = "windows") {
        "windows".into()
    } else if cfg!(target_os = "macos") {
        "macos".into()
    } else {
        "linux".into()
    }
}
