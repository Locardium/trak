//! Audio file transfer (Phase 5.4).
//!
//! The hard requirement of all of Phase 5 is that music must never be lost.
//! That's not achieved by being careful, it's achieved by a path that has no
//! way to lose anything:
//!
//! ```text
//! 1. request the file from byte N (N = whatever is already in the .part)
//! 2. write to  <sway>/incoming/<hash>.part
//! 3. network cut / app killed -> the .part survives, resumes from there
//! 4. complete -> blake3 of the WHOLE .part
//!      different  -> the .part is deleted, the library was never touched
//!      same       -> rename() to the final destination (atomic, same filesystem)
//! 5. only then, the row in the DB
//! ```
//!
//! Invariants:
//! - The final destination is **never** written live: everything goes through the .part.
//! - If the destination exists with different content, the name is disambiguated. An
//!   existing audio file is never overwritten.
//! - The whole file is verified, not just what just arrived: a .part with a
//!   corrupt prefix from a previous run also has to fail.

use crate::wire::{Msg, Session};
use anyhow::{anyhow, Result};
use rusqlite::OptionalExtension;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Size of each raw payload. The channel internally splits it into 64 KiB
/// frames (Noise's cap); this is just how much is read from disk per round.
const CHUNK: usize = 1024 * 1024;

/// Half-finished downloads. Deliberately kept on the same filesystem as the
/// managed folder (`rename()` is only atomic within one filesystem, and on
/// Android the app folder and the music folder can be on different
/// volumes) — see `internal_dir::internal_base`.
pub fn incoming_dir(music_dir: &Path) -> PathBuf {
    crate::internal_dir::internal_base(music_dir).join("incoming")
}

fn part_path(music_dir: &Path, hash: &str) -> PathBuf {
    incoming_dir(music_dir).join(format!("{hash}.part"))
}

/// A push has its own partial: it always starts from scratch, and sharing the
/// pull's `<hash>.part` meant throwing away a half-done download of the same
/// file (or two writers on one file).
fn push_part_path(music_dir: &Path, hash: &str) -> PathBuf {
    incoming_dir(music_dir).join(format!("{hash}.push.part"))
}

/// Pushes being received right now in this process, by partial path.
///
/// The server takes several devices at once, and two of them can push the
/// same file at the same time (2.6): it went up twice, both wrote the same
/// partial, and the hash check failed. The second one is now told "not
/// wanted" (`Msg::BlobPushAck`) and skips it; the next sync sees it there.
static RECEIVING: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

/// The right to receive one pushed file. Released on drop, whatever happens.
pub struct PushClaim(PathBuf);

impl Drop for PushClaim {
    fn drop(&mut self) {
        RECEIVING.lock().unwrap_or_else(|e| e.into_inner()).retain(|p| p != &self.0);
    }
}

/// `None` if this file is already coming in through another session.
pub fn claim_push(music_dir: &Path, hash: &str) -> Option<PushClaim> {
    let part = push_part_path(music_dir, hash);
    let mut busy = RECEIVING.lock().unwrap_or_else(|e| e.into_inner());
    if busy.contains(&part) {
        return None;
    }
    busy.push(part.clone());
    Some(PushClaim(part))
}

/// How much of this file has already been downloaded.
fn resume_offset(music_dir: &Path, hash: &str) -> u64 {
    std::fs::metadata(part_path(music_dir, hash))
        .map(|m| m.len())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Sending side
// ---------------------------------------------------------------------------

/// Sends `path` starting at `offset`. The receiver already knows the expected hash.
/// `progress(sent, total)` goes after every chunk, like on the receiving side:
/// without it the sending device's bar sat at 0 for the whole file.
pub fn send_file(
    sess: &mut Session,
    path: &Path,
    offset: u64,
    hash: &str,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<()> {
    let mut f = std::fs::File::open(path)?;
    let total = f.metadata()?.len();
    if offset > total {
        return Err(anyhow!("offset {offset} is past the end of the file ({total})"));
    }
    f.seek(SeekFrom::Start(offset))?;
    sess.send(&Msg::BlobStart {
        size: total - offset,
    })?;

    let mut buf = vec![0u8; CHUNK];
    let mut sent = offset;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        sess.send_bytes(&buf[..n])?;
        sent += n as u64;
        progress(sent, total);
    }
    sess.send(&Msg::BlobEnd {
        hash: hash.to_string(),
    })
}

// ---------------------------------------------------------------------------
// Receiving side
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct Received {
    pub path: PathBuf,
    pub bytes: u64,
}

/// Receives a file that was already requested (or pushed) and leaves it in the
/// managed folder. Returns the final path.
///
/// `progress(received, total)` is called often: with 40 MB files over a home
/// network, without this the UI looks frozen.
/// `mark_expected` is called with the destination **before** the rename. The
/// managed folder's watcher uses that to avoid auto-importing what sync just
/// brought in: if it imported it, it would create a row with a new uid and
/// overwrite the metadata synced from the file's tags.
///
/// `push`: the other side sends on its own, from scratch, without knowing
/// what we have. It goes to its own partial (`push_part_path`), and an old
/// one is discarded: stacking the whole file on top of it gives a longer
/// file, a different hash, and an entire transfer wasted that repeats on
/// every sync. The caller holds a `claim_push` for it. A pull continues its
/// `.part` instead: the request carried that offset.
pub fn receive_file(
    sess: &mut Session,
    music_dir: &Path,
    hash: &str,
    filename: &str,
    push: bool,
    progress: &mut dyn FnMut(u64, u64),
    mark_expected: &mut dyn FnMut(&Path),
) -> Result<Received> {
    let incoming = incoming_dir(music_dir);
    std::fs::create_dir_all(&incoming)?;
    let part = if push { push_part_path(music_dir, hash) } else { part_path(music_dir, hash) };
    if push && part.exists() {
        log::debug!("[sync] discarding a stale partial of {hash}");
        let _ = std::fs::remove_file(&part);
    }

    let expected = match sess.recv()? {
        Msg::BlobStart { size } => size,
        Msg::BlobError { reason } => return Err(anyhow!(reason)),
        other => return Err(anyhow!("expected BlobStart, got {other:?}")),
    };

    // Append: what was already there is kept; that's why the request carried an offset.
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&part)?;
    let already = f.metadata()?.len();
    let total = already + expected;
    let mut got = already;
    progress(got, total);

    while got < total {
        let chunk = sess.recv_bytes()?;
        if chunk.is_empty() {
            return Err(anyhow!("the sender stopped at {got} of {total} bytes"));
        }
        f.write_all(&chunk)?;
        got += chunk.len() as u64;
        progress(got, total);
    }
    // To disk before considering the download good: if power is cut here, what's
    // left in the .part has to be what was actually written.
    f.flush()?;
    f.sync_all()?;
    drop(f);

    match sess.recv()? {
        Msg::BlobEnd { hash: sent } if sent == hash => {}
        Msg::BlobEnd { hash: sent } => {
            let _ = std::fs::remove_file(&part);
            return Err(anyhow!("the sender reports having sent {sent}, {hash} was requested"));
        }
        other => return Err(anyhow!("expected BlobEnd, got {other:?}")),
    }

    // The WHOLE file is verified, not just what just arrived: a .part with a
    // corrupt prefix from a previous run also has to fail.
    let actual = crate::hashing::hash_file(&part)?;
    if actual != hash {
        // The library was never touched: the only thing lost is the download.
        let _ = std::fs::remove_file(&part);
        return Err(anyhow!("hash mismatch (expected {hash}, got {actual})"));
    }

    let dest = crate::import::managed_dest_for(music_dir, filename, got);
    mark_expected(&dest);
    // `rename` within the same filesystem is atomic: the file appears whole or
    // doesn't appear at all. It never ends up half-done in the library.
    std::fs::rename(&part, &dest)?;
    Ok(Received {
        path: dest,
        bytes: got,
    })
}

/// Requests a file and receives it, resuming if something was already downloaded.
/// `(index, of)` is its place in the run, sent along so the other side's
/// progress reads "3 of 128" too.
pub fn pull_file(
    sess: &mut Session,
    music_dir: &Path,
    hash: &str,
    filename: &str,
    (index, of): (usize, usize),
    progress: &mut dyn FnMut(u64, u64),
    mark_expected: &mut dyn FnMut(&Path),
) -> Result<Received> {
    let offset = resume_offset(music_dir, hash);
    if offset > 0 {
        log::info!("[sync] resuming {filename} from {offset} bytes");
    }
    sess.send(&Msg::BlobReq {
        hash: hash.to_string(),
        offset,
        index,
        of,
    })?;
    receive_file(sess, music_dir, hash, filename, false, progress, mark_expected)
}

// ---------------------------------------------------------------------------
// Adding to the library
// ---------------------------------------------------------------------------

/// Adds a received file, keeping the `uid` from the other device.
///
/// The uid has to be the same on both sides or the next sync wouldn't
/// recognize it's the same song: playlists and tombstones reference it. This
/// is why the normal import path isn't used, since that generates a new one
/// and rereads the file's tags.
#[allow(clippy::too_many_arguments)]
pub fn insert_received(
    conn: &rusqlite::Connection,
    dest: &Path,
    uid: &str,
    hash: &str,
    size: u64,
    title: &str,
    artist: &str,
    album: &str,
    genre: &str,
    duration_ms: i64,
    bpm: Option<i64>,
    updated_at: i64,
) -> rusqlite::Result<i64> {
    // Touched on the other side after it was deleted here (kept there, 2.7):
    // the delete lost and its tombstone is stale.
    conn.execute(
        "DELETE FROM tombstones WHERE entity = 'track' AND uid = ?1 AND deleted_at < ?2",
        rusqlite::params![uid, updated_at],
    )?;
    let (_, mtime) = crate::hashing::file_stamp(dest).unwrap_or((0, 0));
    let rel = dest
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    // If there's already a row with this uid, another one CANNOT be inserted:
    // `uid` has a unique index and the INSERT blows up with "UNIQUE constraint
    // failed: tracks.uid". And that's not a cosmetic detail: the error kills
    // the whole session, so sync fails, retries on its own, and fails again —
    // nothing ever gets copied.
    //
    // This happens more often than it seems: while the hash backfill hasn't
    // set `content_hash` on a row yet, the other side doesn't see it as
    // present and sends the file that's already here.
    let existing: Option<(i64, String)> = conn
        .query_row(
            "SELECT id, path FROM tracks WHERE uid = ?1",
            [uid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((id, old_path)) = existing {
        let old = Path::new(&old_path);
        if old != dest && old.exists() {
            // There was already a file for this track. We have to keep just
            // one —the uid is unique, the row points to one path— and above
            // all **record a hash that matches the file that remains**.
            //
            // Before this discarded the received copy without touching the
            // hash. If the local row had no hash (backfill pending), the
            // other side kept seeing that we were "missing" it and sent it
            // again: the same track traveling on every sync, forever.
            let same = crate::hashing::hash_file(old).map(|h| h == hash).unwrap_or(false);
            if same {
                // Same content: the transfer was redundant.
                let _ = std::fs::remove_file(dest);
                conn.execute(
                    "UPDATE tracks SET content_hash = ?1, size_bytes = ?2,
                            local_state = 'present' WHERE id = ?3",
                    rusqlite::params![hash, size as i64, id],
                )?;
                return Ok(id);
            }
            // Different content under the same track (different encoding,
            // different edit). The one that just arrived wins, and the one
            // that was there **goes to the library's trash**, not to the void:
            // it's still the user's music and is recoverable for 30 days.
            // Keeping the old one wouldn't work — the other side would send
            // its own again on every sync.
            // `dest` always hangs off the managed folder (`managed_dest_for`
            // chose it), so its parent IS the managed folder.
            if let Some(managed) = dest.parent() {
                match crate::trash::move_to_trash(managed, old) {
                    Ok(p) => {
                        log::info!("[sync] replaced, previous one moved to trash: {}", p.display())
                    }
                    Err(e) => log::warn!("[sync] could not store {}: {e}", old.display()),
                }
            }
        }
        // The row points to the file that just arrived: either it had none
        // (freed by selective sync, deleted by hand) or the one it had was
        // different content and was already archived above.
        conn.execute(
            "UPDATE tracks SET path = ?1, rel_path = ?2, content_hash = ?3,
                    size_bytes = ?4, mtime_ms = ?5, local_state = 'present'
             WHERE id = ?6",
            rusqlite::params![
                dest.to_string_lossy(),
                rel,
                hash,
                size as i64,
                mtime,
                id
            ],
        )?;
        return Ok(id);
    }

    conn.execute(
        "INSERT INTO tracks (path, title, artist, album, genre, duration_ms, bpm,
                             uid, content_hash, rel_path, size_bytes, mtime_ms,
                             updated_at, local_state)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 'present')
         ON CONFLICT(path) DO UPDATE SET
            content_hash = excluded.content_hash,
            size_bytes   = excluded.size_bytes,
            mtime_ms     = excluded.mtime_ms,
            local_state  = 'present'",
        rusqlite::params![
            dest.to_string_lossy(),
            title,
            artist,
            album,
            genre,
            duration_ms,
            bpm,
            uid,
            hash,
            rel,
            size as i64,
            mtime,
            updated_at
        ],
    )?;
    conn.query_row(
        "SELECT id FROM tracks WHERE path = ?1",
        [dest.to_string_lossy()],
        |r| r.get(0),
    )
}

/// Resolves the local file that corresponds to a hash, so it can be served.
pub fn path_for_hash(conn: &rusqlite::Connection, hash: &str) -> Option<PathBuf> {
    conn.query_row(
        "SELECT path FROM tracks WHERE content_hash = ?1 AND local_state = 'present' LIMIT 1",
        [hash],
        |r| r.get::<_, String>(0),
    )
    .ok()
    .map(PathBuf::from)
    .filter(|p| p.exists())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::generate_keypair;
    use std::net::{TcpListener, TcpStream};

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "sway-xfer-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Two ends of an encrypted session over loopback.
    fn pair() -> (Session, Session) {
        let (a, _) = generate_keypair().unwrap();
        let (b, _) = generate_keypair().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            Session::accept(s, &b).unwrap()
        });
        let client = Session::connect(TcpStream::connect(addr).unwrap(), &a).unwrap();
        (client, server.join().unwrap())
    }

    fn sample(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i % 251) as u8).collect()
    }

    /// A push sends the file from scratch: the other side doesn't know what
    /// we have half-downloaded. If the old `.part` were kept, the whole file
    /// would get stacked on top of it — longer, a different hash, and a
    /// complete transfer wasted that repeats on every sync.
    #[test]
    fn a_stale_partial_does_not_poison_a_push() {
        let src_dir = tmpdir("push-src");
        let dst_dir = tmpdir("push-dst");
        let src = src_dir.join("track.flac");
        let data = sample(CHUNK + 500);
        std::fs::write(&src, &data).unwrap();
        let hash = crate::hashing::hash_file(&src).unwrap();

        // A partial from a previous download that got cut off is left over.
        std::fs::create_dir_all(incoming_dir(&dst_dir)).unwrap();
        std::fs::write(part_path(&dst_dir, &hash), &data[..1000]).unwrap();

        let (mut client, mut server) = pair();
        let h = hash.clone();
        let src2 = src.clone();
        let sender = std::thread::spawn(move || {
            send_file(&mut server, &src2, 0, &h, &mut |_, _| {}).unwrap();
        });
        let got = receive_file(
            &mut client,
            &dst_dir,
            &hash,
            "track.flac",
            true,
            &mut |_, _| {},
            &mut |_| {},
        )
        .expect("the old partial cannot ruin the push");
        sender.join().unwrap();

        assert_eq!(std::fs::read(&got.path).unwrap(), data);
        // And the half-done download is still there to resume.
        assert_eq!(std::fs::read(part_path(&dst_dir, &hash)).unwrap(), &data[..1000]);
        std::fs::remove_dir_all(&src_dir).ok();
        std::fs::remove_dir_all(&dst_dir).ok();
    }

    /// `uid` has a unique index: receiving a track that already exists here
    /// cannot end in "UNIQUE constraint failed: tracks.uid". That error used
    /// to kill the whole session, so sync would fail, retry on its own, and
    /// fail again — nothing ever got copied. It happens whenever the hash
    /// backfill hasn't yet set `content_hash` on the row: the other side
    /// doesn't see it as present and sends the file again.
    #[test]
    fn receiving_a_track_that_already_exists_here_does_not_break_the_session() {
        let dir = tmpdir("dup");
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::init_schema(&conn).unwrap();

        let mine = dir.join("i-already-have-it.flac");
        std::fs::write(&mine, b"audio").unwrap();
        conn.execute(
            "INSERT INTO tracks (path, uid, rel_path, local_state)
             VALUES (?1, 'uid-1', 'i-already-have-it.flac', 'present')",
            [mine.to_string_lossy()],
        )
        .unwrap();

        // The same content arrives with a different filename.
        let hash = crate::hashing::hash_file(&mine).unwrap();
        let incoming = dir.join("i-already-have-it (2).flac");
        std::fs::write(&incoming, b"audio").unwrap();
        let id = insert_received(&conn, &incoming, "uid-1", &hash, 5, "T", "A", "", "", 0, None, 10)
            .expect("cannot fail because of the unique index");

        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "a single row for that uid");
        let (path, stored): (String, Option<String>) = conn
            .query_row("SELECT path, content_hash FROM tracks WHERE id = ?1", [id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(path, mine.to_string_lossy(), "keeps the file it already had");
        assert!(!incoming.exists(), "and the redundant copy isn't left lying around");
        // What closed the loop: without a recorded hash, the other side sees
        // us as "missing" it and sends it again on every sync.
        assert_eq!(stored.as_deref(), Some(hash.as_str()));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Same track (same uid) with different bytes on each side. We have to
    /// keep just one or sync never converges — but the loser **is not
    /// destroyed**: it goes to the library's trash.
    #[test]
    fn a_different_encoding_of_the_same_track_does_not_loop_and_does_not_lose_the_old_file() {
        let dir = tmpdir("replace");
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::init_schema(&conn).unwrap();

        let mine = dir.join("track.flac");
        std::fs::write(&mine, b"old version").unwrap();
        conn.execute(
            "INSERT INTO tracks (path, uid, rel_path, local_state)
             VALUES (?1, 'uid-1', 'track.flac', 'present')",
            [mine.to_string_lossy()],
        )
        .unwrap();

        let incoming = dir.join("track (2).flac");
        std::fs::write(&incoming, b"new version").unwrap();
        let hash = crate::hashing::hash_file(&incoming).unwrap();
        let id = insert_received(&conn, &incoming, "uid-1", &hash, 13, "T", "A", "", "", 0, None, 10)
            .unwrap();

        let (path, stored): (String, Option<String>) = conn
            .query_row("SELECT path, content_hash FROM tracks WHERE id = ?1", [id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(path, incoming.to_string_lossy(), "the one that arrived wins");
        assert_eq!(stored.as_deref(), Some(hash.as_str()), "and its hash gets recorded");
        assert!(!mine.exists(), "the old one leaves the library");
        let recoverable = std::fs::read_dir(crate::trash::trash_dir(&dir))
            .unwrap()
            .flatten()
            .any(|e| std::fs::read(e.path()).unwrap() == b"old version");
        assert!(recoverable, "but it still exists in the trash");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Full cycle of selective sync: space is freed (the row becomes
    /// `absent`) and when the playlist is re-marked, the file comes back
    /// **to the same row**. Reinserting with the same uid and a different
    /// path used to collide with `uid`'s unique index, and the track came
    /// back duplicated or didn't come back at all.
    #[test]
    fn a_freed_track_comes_back_to_the_same_row() {
        let dir = tmpdir("recover");
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::init_schema(&conn).unwrap();

        let first = dir.join("track.flac");
        std::fs::write(&first, b"audio").unwrap();
        insert_received(&conn, &first, "uid-1", "h1", 5, "T", "A", "", "", 0, None, 10).unwrap();

        // Freed: the row stays, the file doesn't.
        conn.execute(
            "UPDATE tracks SET local_state = 'absent' WHERE uid = 'uid-1'",
            [],
        )
        .unwrap();
        std::fs::remove_file(&first).unwrap();

        // It comes back, with a different name too (the destination is
        // disambiguated if needed).
        let again = dir.join("track (2).flac");
        std::fs::write(&again, b"audio").unwrap();
        insert_received(&conn, &again, "uid-1", "h1", 5, "T", "A", "", "", 0, None, 10).unwrap();

        let (n, state, path): (i64, String, String) = conn
            .query_row(
                "SELECT COUNT(*), MAX(local_state), MAX(path) FROM tracks WHERE uid = 'uid-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(n, 1, "a single row, not a duplicate");
        assert_eq!(state, "present");
        assert_eq!(path, again.to_string_lossy());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn transfers_and_verifies_a_file() {
        let src_dir = tmpdir("src");
        let dst_dir = tmpdir("dst");
        let src = src_dir.join("track.flac");
        // More than one chunk, to exercise the chunking.
        let data = sample(CHUNK * 2 + 1234);
        std::fs::write(&src, &data).unwrap();
        let hash = crate::hashing::hash_file(&src).unwrap();

        let (mut client, mut server) = pair();
        let h = hash.clone();
        let sender = std::thread::spawn(move || {
            match server.recv().unwrap() {
                Msg::BlobReq { offset, .. } => {
                    send_file(&mut server, &src, offset, &h, &mut |_, _| {}).unwrap();
                }
                other => panic!("unexpected: {other:?}"),
            }
        });

        let got = pull_file(&mut client, &dst_dir, &hash, "track.flac", (1, 1), &mut |_, _| {}, &mut |_| {}).unwrap();
        sender.join().unwrap();

        assert_eq!(std::fs::read(&got.path).unwrap(), data);
        assert_eq!(got.bytes as usize, data.len());
        // The .part was consumed: no leftover junk.
        assert!(!part_path(&dst_dir, &hash).exists());
        std::fs::remove_dir_all(&src_dir).ok();
        std::fs::remove_dir_all(&dst_dir).ok();
    }

    /// A sender that sends bytes that aren't the ones for the requested
    /// hash —bug, corruption in transit, or bad faith— can't get anything
    /// into the library. It's the only check that matters: the hash declared
    /// in BlobEnd proves nothing, what's verified is the bytes received.
    #[test]
    fn content_that_does_not_match_the_requested_hash_is_rejected() {
        let src_dir = tmpdir("bad-src");
        let dst_dir = tmpdir("bad-dst");
        // What the receiver wants...
        let wanted = src_dir.join("good.flac");
        std::fs::write(&wanted, sample(5000)).unwrap();
        let wanted_hash = crate::hashing::hash_file(&wanted).unwrap();
        // ...and what the sender sends instead, claiming it's the same.
        let other = src_dir.join("other.flac");
        std::fs::write(&other, sample(4096)).unwrap();

        let (mut client, mut server) = pair();
        let claimed = wanted_hash.clone();
        let sender = std::thread::spawn(move || {
            let offset = match server.recv().unwrap() {
                Msg::BlobReq { offset, .. } => offset,
                other => panic!("unexpected: {other:?}"),
            };
            send_file(&mut server, &other, offset, &claimed, &mut |_, _| {}).unwrap();
        });

        let err = pull_file(&mut client, &dst_dir, &wanted_hash, "good.flac", (1, 1), &mut |_, _| {}, &mut |_| {})
            .unwrap_err();
        sender.join().unwrap();

        assert!(err.to_string().contains("hash mismatch"), "unexpected error: {err}");
        // Neither the final file nor the .part: the library stayed intact.
        assert!(!dst_dir.join("good.flac").exists());
        assert!(!part_path(&dst_dir, &wanted_hash).exists());
        std::fs::remove_dir_all(&src_dir).ok();
        std::fs::remove_dir_all(&dst_dir).ok();
    }

    /// And if it also lies in BlobEnd, it fails even earlier.
    #[test]
    fn a_wrong_hash_in_blob_end_is_rejected() {
        let src_dir = tmpdir("end-src");
        let dst_dir = tmpdir("end-dst");
        let src = src_dir.join("x.flac");
        std::fs::write(&src, sample(2000)).unwrap();
        let hash = crate::hashing::hash_file(&src).unwrap();

        let (mut client, mut server) = pair();
        let sender = std::thread::spawn(move || {
            let offset = match server.recv().unwrap() {
                Msg::BlobReq { offset, .. } => offset,
                other => panic!("unexpected: {other:?}"),
            };
            send_file(&mut server, &src, offset, &"0".repeat(64), &mut |_, _| {}).unwrap();
        });
        let err = pull_file(&mut client, &dst_dir, &hash, "x.flac", (1, 1), &mut |_, _| {}, &mut |_| {}).unwrap_err();
        sender.join().unwrap();

        assert!(err.to_string().contains("reports having sent"), "error: {err}");
        assert!(!part_path(&dst_dir, &hash).exists());
        std::fs::remove_dir_all(&src_dir).ok();
        std::fs::remove_dir_all(&dst_dir).ok();
    }

    /// A transfer cut off halfway leaves a .part, and the next one starts
    /// from there instead of downloading everything again.
    #[test]
    fn an_interrupted_transfer_resumes_from_where_it_stopped() {
        let src_dir = tmpdir("res-src");
        let dst_dir = tmpdir("res-dst");
        let src = src_dir.join("long.flac");
        let data = sample(CHUNK * 3);
        std::fs::write(&src, &data).unwrap();
        let hash = crate::hashing::hash_file(&src).unwrap();

        // Simulates the cutoff: half the file is already downloaded from a
        // previous run.
        std::fs::create_dir_all(incoming_dir(&dst_dir)).unwrap();
        let half = data.len() / 2;
        std::fs::write(part_path(&dst_dir, &hash), &data[..half]).unwrap();

        let (mut client, mut server) = pair();
        let h = hash.clone();
        let asked = std::thread::spawn(move || {
            let offset = match server.recv().unwrap() {
                Msg::BlobReq { offset, .. } => offset,
                other => panic!("unexpected: {other:?}"),
            };
            send_file(&mut server, &src, offset, &h, &mut |_, _| {}).unwrap();
            offset
        });

        let got = pull_file(&mut client, &dst_dir, &hash, "long.flac", (1, 1), &mut |_, _| {}, &mut |_| {}).unwrap();
        let offset = asked.join().unwrap();

        assert_eq!(offset, half as u64, "had to request only what was missing");
        // And the reassembled file is identical to the original.
        assert_eq!(std::fs::read(&got.path).unwrap(), data);
        std::fs::remove_dir_all(&src_dir).ok();
        std::fs::remove_dir_all(&dst_dir).ok();
    }

    /// A .part with a corrupt prefix (failed disk, dirty cutoff) can't be
    /// detected by size alone: it only fails when the whole file is verified.
    #[test]
    fn a_corrupted_resume_prefix_is_caught_by_the_full_hash() {
        let src_dir = tmpdir("pre-src");
        let dst_dir = tmpdir("pre-dst");
        let src = src_dir.join("x.flac");
        let data = sample(CHUNK + 500);
        std::fs::write(&src, &data).unwrap();
        let hash = crate::hashing::hash_file(&src).unwrap();

        let half = data.len() / 2;
        let mut bad = data[..half].to_vec();
        bad[10] ^= 0xFF; // one byte changed, same size
        std::fs::create_dir_all(incoming_dir(&dst_dir)).unwrap();
        std::fs::write(part_path(&dst_dir, &hash), &bad).unwrap();

        let (mut client, mut server) = pair();
        let h = hash.clone();
        let sender = std::thread::spawn(move || {
            let offset = match server.recv().unwrap() {
                Msg::BlobReq { offset, .. } => offset,
                other => panic!("unexpected: {other:?}"),
            };
            send_file(&mut server, &src, offset, &h, &mut |_, _| {}).unwrap();
        });

        let err =
            pull_file(&mut client, &dst_dir, &hash, "x.flac", (1, 1), &mut |_, _| {}, &mut |_| {}).unwrap_err();
        sender.join().unwrap();
        assert!(err.to_string().contains("hash"), "unexpected error: {err}");
        assert!(!part_path(&dst_dir, &hash).exists(), "the bad .part is discarded");
        std::fs::remove_dir_all(&src_dir).ok();
        std::fs::remove_dir_all(&dst_dir).ok();
    }

    /// An existing file is never overwritten: if the name is taken by
    /// different content, the incoming one is disambiguated.
    #[test]
    fn an_existing_file_with_the_same_name_is_never_overwritten() {
        let src_dir = tmpdir("dup-src");
        let dst_dir = tmpdir("dup-dst");
        let existing = dst_dir.join("same.flac");
        std::fs::write(&existing, b"what was already there").unwrap();

        let src = src_dir.join("same.flac");
        let data = sample(3000);
        std::fs::write(&src, &data).unwrap();
        let hash = crate::hashing::hash_file(&src).unwrap();

        let (mut client, mut server) = pair();
        let h = hash.clone();
        let sender = std::thread::spawn(move || {
            let offset = match server.recv().unwrap() {
                Msg::BlobReq { offset, .. } => offset,
                other => panic!("unexpected: {other:?}"),
            };
            send_file(&mut server, &src, offset, &h, &mut |_, _| {}).unwrap();
        });
        let got = pull_file(&mut client, &dst_dir, &hash, "same.flac", (1, 1), &mut |_, _| {}, &mut |_| {}).unwrap();
        sender.join().unwrap();

        assert_ne!(got.path, existing);
        assert_eq!(std::fs::read(&existing).unwrap(), b"what was already there");
        assert_eq!(std::fs::read(&got.path).unwrap(), data);
        std::fs::remove_dir_all(&src_dir).ok();
        std::fs::remove_dir_all(&dst_dir).ok();
    }
}
