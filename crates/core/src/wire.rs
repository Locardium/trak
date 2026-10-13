//! Encrypted channel between two Sway devices (Phase 5.2).
//!
//! **Noise XX** handshake (`Noise_XX_25519_ChaChaPoly_BLAKE2s`) over TCP.
//! XX exchanges the static keys of both sides, so after the handshake each
//! one knows which public key it's talking to — but not yet whether that key
//! belongs to who it claims to be. That's resolved by the verification code
//! (see `sas_code`), not by the cryptography.
//!
//! Noise was chosen over TLS because there's no PKI to set up: each device
//! just has a keypair and nothing else. No certificates to generate, no
//! chains to validate, no trust store to maintain on Android.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use snow::{Builder, HandshakeState, TransportState};
use std::io::{Read, Write};
use std::net::TcpStream;

const PARAMS: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s";

// ---------------------------------------------------------------------------
// Protocol version (Beta 1 · 2.8)
// ---------------------------------------------------------------------------

/// Version of what travels over the channel: the messages, their fields, and
/// the order they go in.
///
/// **Any change to the protocol bumps this**, so an update never breaks in
/// silence. Additive changes a previous version still understands (a field
/// with `serde(default)`, a message only sent to a peer that knows it) bump
/// only `PROTOCOL`; the newer side gates them with `peer_protocol >= N`.
/// Breaking ones also raise `MIN_PROTOCOL` to the new value. The test
/// `tests::protocol_fingerprint` fails when a message changes without a bump.
pub const PROTOCOL: u32 = 2;

/// The oldest protocol this build can still sync with.
pub const MIN_PROTOCOL: u32 = 2;

/// First frame each side sends after the Noise handshake, before any `Msg`.
/// Not a `Msg` on purpose: with different protocols even `Hello` may not
/// parse, and this has to.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Version {
    protocol: u32,
    min_protocol: u32,
    /// The app's own version, only for telling the user what to update.
    app: String,
}

static APP_VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// The version of the app/server this runs in, sent with the protocol (for
/// messages like "the server runs 1.2.0"). Set once at startup.
pub fn set_app_version(version: &str) {
    let _ = APP_VERSION.set(version.to_string());
}

/// Who has to update when two protocols can't talk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compat {
    Ok,
    PeerOlder,
    WeAreOlder,
}

/// Whether a peer speaking `theirs` (and accepting down to `their_min`) can
/// sync with this build.
pub fn compat(theirs: u32, their_min: u32) -> Compat {
    if theirs < MIN_PROTOCOL {
        Compat::PeerOlder
    } else if PROTOCOL < their_min {
        Compat::WeAreOlder
    } else {
        Compat::Ok
    }
}

/// The two sides can't sync: one of them needs updating. Typed so the app
/// can show "Update needed" instead of retrying forever.
#[derive(Debug, Clone)]
pub struct VersionMismatch {
    /// The peer's protocol; `None` for a build from before versions existed
    /// (it hung up on, or didn't understand, the version frame).
    pub theirs: Option<u32>,
    pub their_app: String,
    pub peer_is_older: bool,
}

impl std::fmt::Display for VersionMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let theirs = match self.theirs {
            Some(p) => format!("protocol {p}"),
            None => "no protocol version".into(),
        };
        let app = if self.their_app.is_empty() {
            String::new()
        } else {
            format!(", app {}", self.their_app)
        };
        let who = if self.peer_is_older { "the other device" } else { "this device" };
        write!(
            f,
            "incompatible versions: the other device speaks {theirs}{app}, this one {PROTOCOL} \
             (min {MIN_PROTOCOL}); {who} needs updating"
        )
    }
}

impl std::error::Error for VersionMismatch {}

/// Noise's cap for a single message (64 KiB), minus the 16-byte tag.
const MAX_NOISE_PAYLOAD: usize = 65535 - 16;
/// Sanity bound on an incoming logical message: keeps a hostile peer (or a
/// bug) from making us allocate unbounded memory.
const MAX_MESSAGE: usize = 64 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

/// How up to date one device is on the other's library.
///
/// `epoch` identifies the run of the side being asked, and it's not
/// decoration: the revision count lives in memory and starts from zero on
/// every startup. Without distinguishing the run, an old mark (revision 57)
/// could match a revision 57 from ANOTHER run, and the one asking would
/// conclude it's up to date exactly when it lost everything that happened in
/// between. With `epoch`, a different run always means "I don't know you,
/// come compare".
///
/// This matters more now that the mark is saved to disk: it used to live in
/// memory and get discarded when the app closed, so the collision was rare;
/// saved, the mark survives for days and colliding becomes a matter of time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mark {
    pub epoch: u64,
    pub rev: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Msg {
    /// First message when the caller isn't paired yet.
    PairRequest {
        uid: String,
        name: String,
        platform: String,
        /// Only for pairing with a screenless device (the file server, Phase
        /// 6.2): there's no one there to compare the six digits, so the proof
        /// that you have the right to pair is a token from its config.
        /// Between two devices with a screen this is `None` and the code is
        /// sent, as usual.
        ///
        /// `default` on purpose: a peer on the previous version doesn't send
        /// it, and its PairRequest still has to be accepted.
        #[serde(default)]
        token: Option<String>,
    },
    /// Decision from the receiving side.
    PairResponse { accepted: bool },
    /// Decision from the calling side. Pairing only goes through if BOTH
    /// accepted: it's enough for one to see a different code to abort.
    PairAck { accepted: bool },
    /// Introduction between devices already paired.
    Hello {
        uid: String,
        name: String,
        platform: String,
        tracks: i64,
        playlists: i64,
        clock_ms: i64,
    },
    /// Orderly disconnect with a reason, so it can be shown in the UI on the
    /// other side.
    Reject { reason: String },
    /// "I removed you from my devices". Pairing is saved on both sides, so
    /// unpairing has to notify the other or it keeps believing they're paired
    /// forever.
    Unpair { uid: String },
    /// "I don't have you in my list." Sent by whoever receives a `Hello` from
    /// a peer it doesn't know; the caller uses it to realize it was unpaired
    /// from the other side.
    NotPaired,
    /// "I'm already syncing with someone else, call me later." Sent instead
    /// of `Hello` by a device with a screen: it runs a single sync at a time
    /// (Fase 2.4), so a phone doesn't pull from the server and take a push
    /// from the PC at once. Not a rejection — the caller retries.
    Busy,
    /// Request and response for the library inventory (Phase 5.3).
    /// `gzip` says whether the requester knows how to read a compressed
    /// inventory.
    ///
    /// The capability travels in the request and not in the greeting because
    /// it's a matter between these two messages and no one else: the
    /// requester says it can read it, the responder gives it that. No session
    /// state to keep up to date in the four places that process a `Hello`.
    ///
    /// `default` on purpose, in both directions. An old peer sends
    /// `{"type":"manifestReq"}` without the field and it reads as `false`, so
    /// it gets the usual inventory. And conversely, an old peer that receives
    /// the extra field ignores it, because for it this variant has no fields
    /// and serde skips what it doesn't know (there's a test that pins this
    /// down). Without this, updating the PC and not the phone would break
    /// the whole sync — quite a bit worse than it being slow.
    ManifestReq {
        #[serde(default)]
        gzip: bool,
    },
    ManifestData {
        manifest: Box<crate::manifest::Manifest>,
    },
    /// The same inventory, compressed (see `manifest::squeeze`).
    ///
    /// It's a separate variant and not a field of `ManifestData` so that an
    /// old peer, which doesn't know it, fails to read it instead of thinking
    /// it received an empty inventory. Only sent to whoever said in its
    /// `Hello` that it can read it.
    ManifestGz { data: Vec<u8> },
    // --- File transfer (Phase 5.4) -----------------------------------------
    //
    // The bytes do NOT travel inside these messages: they go as raw payloads
    // over the same channel (`Session::send_bytes`). Putting them in the JSON
    // would cost 1.33x in base64 or 3-4x as a number array, over gigabytes.
    // The protocol is: BlobStart -> N raw payloads -> BlobEnd.
    /// Request for a file by its hash. `offset` != 0 is a resume.
    BlobReq {
        hash: String,
        offset: u64,
        /// Position of this file in the requester's run ("3 of 128"), so the
        /// side serving it shows the same progress.
        index: usize,
        of: usize,
    },
    /// Starts the transfer: `size` is what's left to send from the offset.
    BlobStart { size: u64 },
    /// End of the transfer; `hash` is that of the COMPLETE file, to verify.
    BlobEnd { hash: String },
    /// Push in the other direction: "take this file". The other side answers
    /// `BlobPushAck`; only if it wants it come the raw payloads and a
    /// BlobEnd, same as above.
    BlobPush {
        track_uid: String,
        hash: String,
        filename: String,
        size: u64,
        title: String,
        artist: String,
        album: String,
        genre: String,
        duration_ms: i64,
        bpm: Option<i64>,
        updated_at: i64,
        /// Same as in `BlobReq`: position in the pusher's run.
        index: usize,
        of: usize,
    },
    /// The other side can't serve what was requested (doesn't have it, can't
    /// read it).
    BlobError { reason: String },
    /// Answer to `BlobPush`, before any byte (2.6): `false` when the file is
    /// already here or another session is receiving it right now.
    BlobPushAck { wanted: bool },
    /// Metadata, playlists, folders and memberships the other side is
    /// missing (Phase 5.5). Sent after the files: a membership for a track
    /// that hasn't arrived yet is ignored, so it's better if it already
    /// exists first.
    MetaPush {
        changes: Box<crate::merge::Changes>,
    },
    /// How many records were actually applied on the other side.
    MetaAck {
        applied: crate::merge::Applied,
    },
    // --- Change notification (Phase 6.9) ------------------------------------
    //
    // Sync is always driven by the caller: the responder answers requests and
    // decides nothing. Against the file server that leaves a gap — a change
    // made on the phone reaches the server right away, but the PC has no way
    // to find out, because nobody tells it. These three messages are how it
    // finds out without polling every so often: the caller leaves a
    // connection open and the responder replies when something happens.
    /// "Tell me when your library changes." After sending it, the caller
    /// sends nothing else: it listens.
    ///
    /// `since` is the last revision the caller knows about. It's what makes
    /// reconnecting cheap: on a phone the connection doesn't survive half a
    /// minute —it switches from wifi to mobile data, the carrier's NAT cuts
    /// off anything that's quiet—, so it reconnects constantly. Without this
    /// number, every reconnection would have to drag along a full refresh
    /// just in case; with it, the responder knows whether something was
    /// actually missed and replies `Changed` on the spot, or parks. `None` =
    /// "I don't know anything": park from now on.
    Watch { since: Option<Mark> },
    /// There were changes. Ends the wait; what follows is a normal sync, on
    /// its own connection. The mark is where to keep waiting from afterward.
    Changed { mark: Mark },
    /// Heartbeat while nothing is happening. A connection that stays quiet
    /// for hours gets cut by anything in between (a proxy, the router's NAT)
    /// and the waiting side doesn't notice until it needs to.
    ///
    /// Carries the mark so the waiting side keeps it up to date without
    /// asking: if it gets cut after hours parked, it reconnects asking for
    /// the latest and not for yesterday's.
    Ping { mark: Mark },
    /// Orderly close of the session.
    Bye,
}

// ---------------------------------------------------------------------------
// Verification code
// ---------------------------------------------------------------------------

/// Six digits derived from the handshake hash.
///
/// That hash depends on the ephemeral and static keys of both sides, so a
/// man-in-the-middle ends up with TWO different sessions and two different
/// hashes: it can't make the two codes match. Having the user compare the
/// numbers on the two screens is what turns "there's an encrypted channel
/// with someone" into "there's an encrypted channel with this device".
pub fn sas_code(handshake_hash: &[u8]) -> String {
    let mut n: u32 = 0;
    for b in handshake_hash.iter().take(4) {
        n = (n << 8) | *b as u32;
    }
    format!("{:06}", n % 1_000_000)
}

// ---------------------------------------------------------------------------
// Static keys
// ---------------------------------------------------------------------------

/// Generates a new keypair (once per device).
pub fn generate_keypair() -> Result<(Vec<u8>, Vec<u8>)> {
    let kp = Builder::new(PARAMS.parse()?).generate_keypair()?;
    Ok((kp.private, kp.public))
}

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

pub struct Session {
    stream: TcpStream,
    noise: TransportState,
    /// The other side's static public key, learned during the handshake.
    pub peer_pubkey: Vec<u8>,
    /// Code to show on screen. Both sides compute the same one.
    pub code: String,
    /// The peer's protocol, for gating features newer than `MIN_PROTOCOL`.
    /// Always compatible: a session with an incompatible peer isn't returned.
    pub peer_protocol: u32,
    /// The peer's app version, as it declared it.
    pub peer_app: String,
}

impl Session {
    /// Calling side.
    pub fn connect(stream: TcpStream, private_key: &[u8]) -> Result<Self> {
        tune(&stream);
        let mut hs = Builder::new(PARAMS.parse()?)
            .local_private_key(private_key)?
            .build_initiator()?;
        let mut buf = vec![0u8; 65535];

        // XX is three messages: -> e | <- e ee s es | -> s se
        let n = hs.write_message(&[], &mut buf)?;
        write_frame(&stream, &buf[..n])?;
        let msg = read_frame(&stream)?;
        hs.read_message(&msg, &mut buf)?;
        let n = hs.write_message(&[], &mut buf)?;
        write_frame(&stream, &buf[..n])?;

        Self::finish(stream, hs)
    }

    /// Accepting side.
    pub fn accept(stream: TcpStream, private_key: &[u8]) -> Result<Self> {
        tune(&stream);
        let mut hs = Builder::new(PARAMS.parse()?)
            .local_private_key(private_key)?
            .build_responder()?;
        let mut buf = vec![0u8; 65535];

        let msg = read_frame(&stream)?;
        hs.read_message(&msg, &mut buf)?;
        let n = hs.write_message(&[], &mut buf)?;
        write_frame(&stream, &buf[..n])?;
        let msg = read_frame(&stream)?;
        hs.read_message(&msg, &mut buf)?;

        Self::finish(stream, hs)
    }

    fn finish(stream: TcpStream, hs: HandshakeState) -> Result<Self> {
        // The handshake hash has to be read BEFORE moving to transport mode:
        // `into_transport_mode` consumes the state.
        let code = sas_code(hs.get_handshake_hash());
        let peer_pubkey = hs
            .get_remote_static()
            .ok_or_else(|| anyhow!("the peer did not send its static key"))?
            .to_vec();
        let mut sess = Self {
            stream,
            noise: hs.into_transport_mode()?,
            peer_pubkey,
            code,
            peer_protocol: 0,
            peer_app: String::new(),
        };
        sess.exchange_versions()?;
        Ok(sess)
    }

    /// Both sides send their version first and then read the other's, so
    /// neither waits on the other. Done here and not by each caller: nothing
    /// can skip it.
    fn exchange_versions(&mut self) -> Result<()> {
        let mine = Version {
            protocol: PROTOCOL,
            min_protocol: MIN_PROTOCOL,
            app: APP_VERSION.get().cloned().unwrap_or_default(),
        };
        self.send_bytes(&serde_json::to_vec(&mine)?)?;
        let theirs: Version = match self.recv_bytes() {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(v) => v,
                // Something else first (a `Hello`): a build from before
                // versions existed.
                Err(_) => return Err(Self::too_old().into()),
            },
            // A build from before versions can't parse our version frame and
            // hangs up right after the handshake.
            Err(e) if crate::engine::is_disconnect(&e) => return Err(Self::too_old().into()),
            Err(e) => return Err(e),
        };
        self.peer_protocol = theirs.protocol;
        self.peer_app = theirs.app.clone();
        match compat(theirs.protocol, theirs.min_protocol) {
            Compat::Ok => Ok(()),
            c => Err(VersionMismatch {
                theirs: Some(theirs.protocol),
                their_app: theirs.app,
                peer_is_older: c == Compat::PeerOlder,
            }
            .into()),
        }
    }

    fn too_old() -> VersionMismatch {
        VersionMismatch {
            theirs: None,
            their_app: String::new(),
            peer_is_older: true,
        }
    }

    pub fn send(&mut self, msg: &Msg) -> Result<()> {
        self.send_bytes(&serde_json::to_vec(msg)?)
    }

    pub fn recv(&mut self) -> Result<Msg> {
        let bytes = self.recv_bytes()?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    /// Whether the other side sent something not yet read, or hung up,
    /// without blocking. While a person looks at the pairing code the peer
    /// is expected to stay silent: anything it sends in the meantime (a
    /// `Reject` because its own dialog was closed, its answer arriving
    /// first) means the wait is over.
    pub fn peer_has_spoken(&self) -> bool {
        if self.stream.set_nonblocking(true).is_err() {
            return false;
        }
        let mut byte = [0u8; 1];
        let peeked = self.stream.peek(&mut byte);
        let _ = self.stream.set_nonblocking(false);
        match peeked {
            // Ok(0) is a hang-up, Ok(n) is data: both end the wait.
            Ok(_) => true,
            Err(e) => e.kind() != std::io::ErrorKind::WouldBlock,
        }
    }

    /// A logical message goes as a header frame with the total length, then
    /// the data frames. Noise can't encrypt more than 64 KiB at once, and
    /// manifests (5.3) and audio chunks (5.4) go well past that size, so the
    /// chunking is handled here, in one place.
    pub fn send_bytes(&mut self, data: &[u8]) -> Result<()> {
        let mut buf = vec![0u8; 65535];
        let header = (data.len() as u32).to_be_bytes();
        let n = self.noise.write_message(&header, &mut buf)?;
        write_frame(&self.stream, &buf[..n])?;
        for chunk in data.chunks(MAX_NOISE_PAYLOAD) {
            let n = self.noise.write_message(chunk, &mut buf)?;
            write_frame(&self.stream, &buf[..n])?;
        }
        // No `flush`: on a raw `TcpStream` it's a no-op, and having it here
        // suggested there was a buffer of ours pending to be flushed. There
        // isn't — every frame already went out with its own `write_all`.
        Ok(())
    }

    pub fn recv_bytes(&mut self) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; 65535];
        let frame = read_frame(&self.stream)?;
        let n = self.noise.read_message(&frame, &mut buf)?;
        if n != 4 {
            return Err(anyhow!("invalid header ({n} bytes)"));
        }
        let total = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
        if total > MAX_MESSAGE {
            return Err(anyhow!("message too large ({total} bytes)"));
        }
        let mut out = Vec::with_capacity(total);
        while out.len() < total {
            let frame = read_frame(&self.stream)?;
            let n = self.noise.read_message(&frame, &mut buf)?;
            out.extend_from_slice(&buf[..n]);
            if n == 0 {
                return Err(anyhow!("the peer cut the message in half"));
            }
        }
        out.truncate(total);
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// Socket framing: [u32 length][bytes]
// ---------------------------------------------------------------------------

/// Header and body in **one** write.
///
/// In two, the header's 4 bytes go out as their own packet and end up
/// exposed to Nagle's delay. A track is hundreds of frames, so it's worth not
/// paying for that. Copying 64 KiB into a buffer costs microseconds.
///
/// Watch out for the temptation to give this too much credit: this is NOT
/// what fixed the 12s transfers (that was `opt-level = 0` over
/// ChaCha20-Poly1305, see `Cargo.toml`). With the channel already fast, the
/// difference here is small.
fn write_frame(mut stream: &TcpStream, data: &[u8]) -> Result<()> {
    let mut out = Vec::with_capacity(4 + data.len());
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(data);
    stream.write_all(&out)?;
    Ok(())
}

/// Socket settings, the same on both ends.
///
/// Applied before the handshake on purpose: XX is three small back-and-forth
/// messages, exactly the pattern Nagle penalizes. Same for control messages
/// (`BlobReq`, `MetaAck`, `Bye`): short and strictly sequential, each one
/// waiting for the other's reply before continuing.
///
/// This is a latency improvement for small messages, not throughput for
/// files: the file bottleneck was the unoptimized encryption.
fn tune(stream: &TcpStream) {
    // Failing here isn't a reason to drop the connection: without this it
    // still works, just slower.
    if let Err(e) = stream.set_nodelay(true) {
        log::debug!("[wire] could not disable Nagle: {e}");
    }
}

fn read_frame(mut stream: &TcpStream) -> Result<Vec<u8>> {
    let mut len = [0u8; 4];
    stream.read_exact(&mut len)?;
    if let Some(what) = looks_like_http(&len) {
        return Err(anyhow!(
            "that port is a web server, not a Sway server (it answered {what})"
        ));
    }
    let len = u32::from_be_bytes(len) as usize;
    if len > 65535 {
        return Err(anyhow!("invalid frame ({len} bytes)"));
    }
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf)?;
    Ok(buf)
}

/// The first four bytes, read as text, when they're the start of something
/// HTTP.
///
/// Pointing at the wrong port is THE error of this protocol, and it happens
/// on both sides: the app against a web proxy (which answers `HTTP/1.1 400`),
/// or a browser against the server (which sends `GET `). Both cases used to
/// end up as "invalid frame (1213486160 bytes)", which is the length of the
/// four-letter ASCII message read as a number — a number that means nothing
/// to anyone, let alone that the port belongs to a web server.
fn looks_like_http(head: &[u8; 4]) -> Option<&'static str> {
    match head {
        b"HTTP" => Some("HTTP"),
        b"GET " => Some("GET"),
        b"POST" => Some("POST"),
        b"HEAD" => Some("HEAD"),
        b"PUT " => Some("PUT"),
        b"OPTI" => Some("OPTIONS"),
        b"DELE" => Some("DELETE"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use std::net::{TcpListener, TcpStream};

    /// Pointing at a web proxy's port is THE error of this protocol. The
    /// message has to say that, not a four-byte number read as a length.
    #[test]
    fn against_a_web_server_the_error_says_so() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let web = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            // Like a real proxy: first it reads what's sent to it —our
            // handshake, which is garbage to it— and only then replies.
            // Closing before reading would give a socket error and not the
            // one being tested.
            let mut scratch = [0u8; 512];
            let _ = stream.read(&mut scratch);
            let _ = stream.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n");
            // Let the client get a chance to read the response before close.
            std::thread::sleep(std::time::Duration::from_millis(200));
        });

        let stream = TcpStream::connect(addr).unwrap();
        let (private, _) = generate_keypair().unwrap();
        // `unwrap_err` would require `Session` to be Debug just to print it.
        let msg = match Session::connect(stream, &private) {
            Err(e) => e.to_string(),
            Ok(_) => panic!("there can't be a session against a web server"),
        };
        web.join().unwrap();

        assert!(msg.contains("web server"), "useless message: {msg}");
        assert!(msg.contains("HTTP"), "useless message: {msg}");
    }

    /// Every message, every nested type, with every field set. Struct
    /// literals on purpose: a new field doesn't compile until it's added
    /// here, and then it changes the fingerprint.
    fn every_message() -> Vec<Msg> {
        use crate::manifest::*;
        use crate::merge::{Applied, Changes};
        let track = TrackEntry {
            uid: "t".into(),
            hash: Some("h".into()),
            size: 1,
            filename: "f.flac".into(),
            title: "T".into(),
            artist: "A".into(),
            album: "B".into(),
            genre: "G".into(),
            duration_ms: 2,
            bpm: Some(3),
            updated_at: 4,
            present: true,
        };
        let playlist = PlaylistEntry {
            uid: "p".into(),
            name: "P".into(),
            kind: "playlist".into(),
            parent_uid: Some("f".into()),
            rank: "a".into(),
            updated_at: 5,
            held: true,
        };
        let membership = Membership {
            playlist_uid: "p".into(),
            track_uid: "t".into(),
            rank: "a".into(),
            added_at: 6,
        };
        let tombstone = Tombstone {
            entity: "track".into(),
            uid: "x".into(),
            deleted_at: 7,
        };
        let scope = ScopeEntry {
            device_uid: "d".into(),
            playlist_uid: "p".into(),
            selected: true,
            updated_at: 8,
        };
        let device_sync = DeviceSync {
            device_uid: "d".into(),
            mode: "selected".into(),
            direction: "both".into(),
            updated_at: 9,
        };
        let manifest = Manifest {
            device_uid: "d".into(),
            tracks: vec![track.clone()],
            playlists: vec![playlist.clone()],
            memberships: vec![membership.clone()],
            tombstones: vec![tombstone.clone()],
            scopes: vec![scope.clone()],
            device_sync: vec![device_sync.clone()],
        };
        let changes = Changes {
            tracks: vec![track],
            playlists: vec![playlist],
            memberships: vec![membership],
            tombstones: vec![tombstone],
            scopes: vec![scope],
            device_sync: vec![device_sync],
        };
        let applied = Applied {
            tracks: 1,
            playlists: 2,
            memberships: 3,
            deleted: 4,
            scope: 5,
            held: 6,
        };
        let mark = Mark { epoch: 1, rev: 2 };
        let all = vec![
            Msg::PairRequest {
                uid: "u".into(),
                name: "n".into(),
                platform: "p".into(),
                token: Some("t".into()),
            },
            Msg::PairResponse { accepted: true },
            Msg::PairAck { accepted: true },
            Msg::Hello {
                uid: "u".into(),
                name: "n".into(),
                platform: "p".into(),
                tracks: 1,
                playlists: 2,
                clock_ms: 3,
            },
            Msg::Reject { reason: "r".into() },
            Msg::Unpair { uid: "u".into() },
            Msg::NotPaired,
            Msg::Busy,
            Msg::ManifestReq { gzip: true },
            Msg::ManifestData { manifest: Box::new(manifest) },
            Msg::ManifestGz { data: vec![1, 2] },
            Msg::BlobReq {
                hash: "h".into(),
                offset: 1,
                index: 2,
                of: 3,
            },
            Msg::BlobStart { size: 1 },
            Msg::BlobEnd { hash: "h".into() },
            Msg::BlobPush {
                track_uid: "t".into(),
                hash: "h".into(),
                filename: "f".into(),
                size: 1,
                title: "T".into(),
                artist: "A".into(),
                album: "B".into(),
                genre: "G".into(),
                duration_ms: 2,
                bpm: Some(3),
                updated_at: 4,
                index: 5,
                of: 6,
            },
            Msg::BlobError { reason: "r".into() },
            Msg::BlobPushAck { wanted: true },
            Msg::MetaPush { changes: Box::new(changes) },
            Msg::MetaAck { applied },
            Msg::Watch { since: Some(mark) },
            Msg::Changed { mark },
            Msg::Ping { mark },
            Msg::Bye,
        ];
        // No wildcard: a new variant doesn't compile until it has a sample
        // above (and a line here).
        for m in &all {
            match m {
                Msg::PairRequest { .. }
                | Msg::PairResponse { .. }
                | Msg::PairAck { .. }
                | Msg::Hello { .. }
                | Msg::Reject { .. }
                | Msg::Unpair { .. }
                | Msg::NotPaired
                | Msg::Busy
                | Msg::ManifestReq { .. }
                | Msg::ManifestData { .. }
                | Msg::ManifestGz { .. }
                | Msg::BlobReq { .. }
                | Msg::BlobStart { .. }
                | Msg::BlobEnd { .. }
                | Msg::BlobPush { .. }
                | Msg::BlobError { .. }
                | Msg::BlobPushAck { .. }
                | Msg::MetaPush { .. }
                | Msg::MetaAck { .. }
                | Msg::Watch { .. }
                | Msg::Changed { .. }
                | Msg::Ping { .. }
                | Msg::Bye => {}
            }
        }
        all
    }

    /// The fingerprint of everything that travels, and the protocol it
    /// belongs to. When it changes, the protocol has to change with it.
    const FINGERPRINT: (u32, &str) =
        (2, "f10c24bafbdd1179c9f24b70700fdb130dd040792efa8090dfabb1ef843c4951");

    /// Changing a message without bumping the protocol breaks an update in
    /// silence: an old peer fails to parse, or worse, parses something else.
    /// This makes the change loud.
    #[test]
    fn protocol_fingerprint() {
        let mut all = String::new();
        for m in every_message() {
            all.push_str(&serde_json::to_string(&m).unwrap());
            all.push('\n');
        }
        let version = Version {
            protocol: 0,
            min_protocol: 0,
            app: String::new(),
        };
        all.push_str(&serde_json::to_string(&version).unwrap());
        let now = blake3::hash(all.as_bytes()).to_hex().to_string();
        assert!(
            FINGERPRINT == (PROTOCOL, now.as_str()),
            "protocol changed: bump PROTOCOL (and MIN_PROTOCOL if it breaks older peers), \
             then update FINGERPRINT in wire.rs to ({PROTOCOL}, \"{now}\")"
        );
    }

    #[test]
    fn compat_says_who_is_behind() {
        assert_eq!(compat(PROTOCOL, MIN_PROTOCOL), Compat::Ok);
        assert_eq!(compat(MIN_PROTOCOL - 1, 0), Compat::PeerOlder);
        assert_eq!(compat(PROTOCOL + 5, PROTOCOL + 1), Compat::WeAreOlder);
        // Newer, but still accepting us: sync with the older one's features.
        assert_eq!(compat(PROTOCOL + 1, MIN_PROTOCOL), Compat::Ok);
    }

    /// A build from before versions speaks first with a `Hello` (it doesn't
    /// send a version frame): the session fails as "the other one is older",
    /// not as a parse error.
    #[test]
    fn a_peer_without_versions_is_reported_as_older() {
        let (a_priv, _) = generate_keypair().unwrap();
        let (b_priv, _) = generate_keypair().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        // The old caller: plain Noise handshake, then a `Hello` right away.
        let old = std::thread::spawn(move || {
            let stream = TcpStream::connect(addr).unwrap();
            let mut hs = Builder::new(PARAMS.parse().unwrap())
                .local_private_key(&a_priv)
                .unwrap()
                .build_initiator()
                .unwrap();
            let mut buf = vec![0u8; 65535];
            let n = hs.write_message(&[], &mut buf).unwrap();
            write_frame(&stream, &buf[..n]).unwrap();
            let msg = read_frame(&stream).unwrap();
            hs.read_message(&msg, &mut buf).unwrap();
            let n = hs.write_message(&[], &mut buf).unwrap();
            write_frame(&stream, &buf[..n]).unwrap();
            let mut s = Session {
                stream,
                noise: hs.into_transport_mode().unwrap(),
                peer_pubkey: vec![],
                code: String::new(),
                peer_protocol: 0,
                peer_app: String::new(),
            };
            let _ = s.send(&Msg::Bye);
            std::thread::sleep(std::time::Duration::from_millis(200));
        });
        let (stream, _) = listener.accept().unwrap();
        let err = match Session::accept(stream, &b_priv) {
            Err(e) => e,
            Ok(_) => panic!("an old peer can't get a session"),
        };
        old.join().unwrap();
        let mismatch = err.downcast_ref::<VersionMismatch>().expect("typed error");
        assert!(mismatch.peer_is_older);
        assert_eq!(mismatch.theirs, None);
    }

    /// Sets up both ends of a session over loopback.
    fn pair() -> (Session, Session) {
        let (a_priv, _) = generate_keypair().unwrap();
        let (b_priv, _) = generate_keypair().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            Session::accept(stream, &b_priv).unwrap()
        });
        let client = Session::connect(TcpStream::connect(addr).unwrap(), &a_priv).unwrap();
        (client, server.join().unwrap())
    }

    /// What the channel gives over loopback, where there's no network to
    /// blame. Measures the real ceiling of encryption + framing. `#[ignore]`
    /// because it's a measurement, not an assertion: run with
    /// `cargo test --release -- --ignored --nocapture channel_throughput`.
    #[test]
    #[ignore]
    fn channel_throughput() {
        const MB: usize = 40;
        let data = vec![7u8; MB * 1024 * 1024];
        let (mut a, mut b) = pair();

        let drain = std::thread::spawn(move || {
            let t = std::time::Instant::now();
            let got = b.recv_bytes().unwrap();
            (got.len(), t.elapsed())
        });

        let t = std::time::Instant::now();
        a.send_bytes(&data).unwrap();
        let send = t.elapsed();
        let (got, recv) = drain.join().unwrap();

        assert_eq!(got, data.len());
        println!(
            "{MB} MB | send {:?} ({:.1} MB/s) | receive {:?} ({:.1} MB/s)",
            send,
            MB as f64 / send.as_secs_f64(),
            recv,
            MB as f64 / recv.as_secs_f64()
        );
    }

    #[test]
    fn both_sides_derive_the_same_code_and_see_each_others_keys() {
        let (a, b) = pair();
        assert_eq!(a.code, b.code);
        assert_eq!(a.code.len(), 6);
        assert!(a.code.chars().all(|c| c.is_ascii_digit()));
        // Each side learned the other's static key, which is what later gets
        // pinned in `devices`.
        assert_ne!(a.peer_pubkey, b.peer_pubkey);
        assert_eq!(a.peer_pubkey.len(), 32);
    }

    /// Pairing waits on a person while watching the socket: silence must read
    /// as "keep waiting", and a message or a hang-up as "stop" — without
    /// consuming the message, which the caller still has to read.
    #[test]
    fn peer_has_spoken_sees_messages_and_hang_ups_without_consuming() {
        let (mut a, b) = pair();
        assert!(!a.peer_has_spoken());
        let mut b = b;
        b.send(&Msg::Reject { reason: "closed".into() }).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(a.peer_has_spoken());
        assert!(matches!(a.recv().unwrap(), Msg::Reject { .. }));
        assert!(!a.peer_has_spoken());
        drop(b);
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(a.peer_has_spoken());
    }

    /// Two different sessions have to produce different codes: if the code
    /// didn't depend on the ephemeral keys, a man-in-the-middle could
    /// replicate the same number on both screens.
    #[test]
    fn code_differs_between_sessions() {
        let (a1, _) = pair();
        let (a2, _) = pair();
        assert_ne!(a1.code, a2.code);
    }

    /// Backward compatibility for the compressed inventory rests on this:
    /// that for an old peer, for which `manifestReq` has no fields, the extra
    /// `gzip` doesn't break anything. If serde rejected it, updating one
    /// device and not the other would break the whole sync.
    #[test]
    fn an_extra_field_does_not_break_a_message_with_no_fields() {
        let with_extra = br#"{"type":"bye","gzip":true,"whatever":42}"#;
        assert!(
            matches!(serde_json::from_slice::<Msg>(with_extra), Ok(Msg::Bye)),
            "an old peer has to be able to ignore what it doesn't know"
        );
    }

    /// And the other way around: an old peer's request, without the field,
    /// reads as "I can't read compressed" — which is the safe answer.
    #[test]
    fn a_request_without_the_field_asks_for_the_usual_inventory() {
        let old = br#"{"type":"manifestReq"}"#;
        match serde_json::from_slice::<Msg>(old) {
            Ok(Msg::ManifestReq { gzip }) => assert!(!gzip),
            other => panic!("could not read the old request: {other:?}"),
        }
    }

    #[test]
    fn messages_round_trip() {
        let (mut a, mut b) = pair();
        a.send(&Msg::PairRequest {
            uid: "u-1".into(),
            name: "PC".into(),
            platform: "windows".into(),
            token: None,
        })
        .unwrap();
        match b.recv().unwrap() {
            Msg::PairRequest { uid, name, .. } => {
                assert_eq!(uid, "u-1");
                assert_eq!(name, "PC");
            }
            other => panic!("unexpected message: {other:?}"),
        }
        b.send(&Msg::PairResponse { accepted: true }).unwrap();
        match a.recv().unwrap() {
            Msg::PairResponse { accepted } => assert!(accepted),
            other => panic!("unexpected message: {other:?}"),
        }
    }

    /// A manifest (5.3) or an audio chunk (5.4) go past the 64 KiB Noise
    /// encrypts at once: the chunking has to be transparent.
    #[test]
    fn payloads_larger_than_one_noise_message_survive() {
        let (mut a, mut b) = pair();
        let big: Vec<u8> = (0..500_000).map(|i| (i % 251) as u8).collect();
        let sender = std::thread::spawn(move || {
            a.send_bytes(&big).unwrap();
            big
        });
        let got = b.recv_bytes().unwrap();
        assert_eq!(got, sender.join().unwrap());
    }
}
