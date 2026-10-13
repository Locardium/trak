//! Applying metadata, playlist, and folder changes (Phase 5.5).
//!
//! 5.4 moved files; this moves the organization: names, folder hierarchy,
//! order, and which track is in which playlist.
//!
//! Rules, all with the same bias — when in doubt, keep:
//!
//! - **Metadata: the newest wins** (`updated_at`). No ties: if they're
//!   equal nothing is touched, so syncing twice in a row doesn't change
//!   anything the second time.
//! - **Playlists: the newest wins**, but only for name, parent, and order.
//!   A playlist only disappears with an explicit tombstone.
//! - **Memberships: union.** A track enters a playlist if any device put
//!   it there; it leaves only with an explicit tombstone. Adding beats a
//!   concurrent removal, which is the right bias when what's at stake is
//!   losing music from a set.
//!
//! - **Deletes: the newest wins**, like everything else. A tombstone only
//!   deletes what wasn't touched after it (`updated_at` for tracks and
//!   playlists, `added_at` for memberships); something edited or re-added
//!   later beats it, and the stale tombstone is dropped. That's also how
//!   "keep" works after a held delete: bumping the clock makes the item win
//!   on every device, whoever passes the tombstone along.
//! - **Deletes made while not linked are held** (Beta 1 · 2.7). A tombstone
//!   older than the pairing with the peer that sent it, for something that
//!   still exists here, is not applied: it waits in `held_deletes` for
//!   the user (delete here too, or keep and send it back). Re-linking is
//!   like linking a new device; it doesn't replay what the other side
//!   deleted in the meantime. Everything else is applied, and the file goes
//!   to the library trash, not into the void.
//!
//!   Until Phase 6.4 there was a per-device policy to ignore or queue
//!   them. It was removed because it filtered by who passed you the
//!   tombstone, so with three devices a delete you rejected on the phone
//!   still got in through the laptop. The hold above doesn't have that
//!   hole: rejecting means "keep", which bumps the clock, and a newer item
//!   beats the same tombstone no matter which device passes it.

use crate::manifest::{Manifest, Membership, PlaylistEntry, ScopeEntry, DeviceSync, TrackEntry, Tombstone};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};


/// Set of changes ready to apply on the other side.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Changes {
    pub tracks: Vec<TrackEntry>,
    pub playlists: Vec<PlaylistEntry>,
    pub memberships: Vec<Membership>,
    pub tombstones: Vec<Tombstone>,
    /// Selective scope (Phase 5.7). `default` so it can still talk to an
    /// older version without breaking.
    #[serde(default)]
    pub scopes: Vec<ScopeEntry>,
    #[serde(default)]
    pub device_sync: Vec<DeviceSync>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Applied {
    pub tracks: usize,
    pub playlists: usize,
    pub memberships: usize,
    /// Things deleted by incoming tombstones (Phase 5.6).
    pub deleted: usize,
    /// Selective scope rows applied (Phase 5.7).
    #[serde(default)]
    pub scope: usize,
    /// Deletions held for the user instead of applied (2.7). Not part of
    /// `total()`: nothing changed here.
    #[serde(default)]
    pub held: usize,
}

impl Applied {
    pub fn total(&self) -> usize {
        self.tracks + self.playlists + self.memberships + self.deleted + self.scope
    }
}

/// What `local` has that `remote` is missing or has gone stale on. This is
/// what needs to be sent so it's caught up.
/// Indexes of the other side's manifest. Built once and queried thousands
/// of times: with `find`/`any` over the lists, comparing two libraries of
/// twenty thousand tracks would mean hundreds of millions of string
/// comparisons — and this runs twice per sync, once per direction.
struct Index<'a> {
    tracks: std::collections::HashMap<&'a str, &'a TrackEntry>,
    playlists: std::collections::HashMap<&'a str, &'a PlaylistEntry>,
    memberships: std::collections::HashMap<String, &'a Membership>,
    /// (entity, uid) -> when it was deleted.
    tombstones: std::collections::HashMap<(&'a str, &'a str), i64>,
    scopes: std::collections::HashMap<(&'a str, &'a str), &'a ScopeEntry>,
    device_sync: std::collections::HashMap<&'a str, &'a DeviceSync>,
}

impl<'a> Index<'a> {
    fn of(m: &'a Manifest) -> Self {
        Index {
            tracks: m.tracks.iter().map(|t| (t.uid.as_str(), t)).collect(),
            playlists: m.playlists.iter().map(|p| (p.uid.as_str(), p)).collect(),
            memberships: m
                .memberships
                .iter()
                .map(|x| (format!("{}:{}", x.playlist_uid, x.track_uid), x))
                .collect(),
            tombstones: m
                .tombstones
                .iter()
                .map(|t| ((t.entity.as_str(), t.uid.as_str()), t.deleted_at))
                .collect(),
            scopes: m
                .scopes
                .iter()
                .map(|e| ((e.device_uid.as_str(), e.playlist_uid.as_str()), e))
                .collect(),
            device_sync: m
                .device_sync
                .iter()
                .map(|d| (d.device_uid.as_str(), d))
                .collect(),
        }
    }
}

pub fn changes_for_peer(local: &Manifest, remote: &Manifest) -> Changes {
    let mut out = Changes::default();
    let they = Index::of(remote);

    for t in &local.tracks {
        let theirs = they.tracks.get(t.uid.as_str()).copied();
        let send = match theirs {
            // Only if ours is strictly newer: with `>=` everything would be
            // resent on every sync without anything actually changing.
            Some(r) => t.updated_at > r.updated_at,
            // If they don't have it, the row travels with the file (5.4).
            // Sending metadata for a track whose file isn't there would
            // create a ghost entry in the other side's library.
            None => false,
        };
        if send {
            out.tracks.push(t.clone());
        }
    }

    for p in &local.playlists {
        if let Some(deleted_at) = they.tombstones.get(&("playlist", p.uid.as_str())) {
            if *deleted_at >= p.updated_at {
                continue;
            }
        }
        match they.playlists.get(p.uid.as_str()) {
            Some(r) if p.updated_at > r.updated_at => out.playlists.push(p.clone()),
            // Only what this side really has. One it merely knows about —
            // outside its scope, learned from a third device — isn't created
            // on the other side (see `PlaylistEntry::held`).
            None if p.held => out.playlists.push(p.clone()),
            _ => {}
        }
    }
    let held: std::collections::HashSet<&str> = local
        .playlists
        .iter()
        .filter(|p| p.held)
        .map(|p| p.uid.as_str())
        .collect();

    // Memberships travel for two different reasons: because the other side
    // is missing the pair (union), or because the ORDER changed. Without
    // the second reason, a reorder never propagated: the pair already
    // existed on both sides and nobody ever resent it.
    let mine_pl: std::collections::HashMap<&str, i64> = local
        .playlists
        .iter()
        .map(|p| (p.uid.as_str(), p.updated_at))
        .collect();
    for m in &local.memberships {
        let key = format!("{}:{}", m.playlist_uid, m.track_uid);
        // A tombstone from the other side only wins if it's LATER than the
        // add. Otherwise, putting a song back into a playlist would undo
        // itself: the old delete would beat the new add, forever.
        if let Some(deleted_at) = they.tombstones.get(&("playlist_track", key.as_str())) {
            if *deleted_at >= m.added_at {
                continue;
            }
        }
        match they.memberships.get(&key) {
            // Same rule: contents only for a playlist that exists over there
            // or that this side holds and is creating over there.
            None if held.contains(m.playlist_uid.as_str())
                || they.playlists.contains_key(m.playlist_uid.as_str()) =>
            {
                out.memberships.push(m.clone())
            }
            None => {}
            // Order belongs to whoever touched the playlist most recently.
            // Memberships have no clock of their own; the playlist does.
            Some(r) if r.rank != m.rank => {
                let mine = mine_pl.get(m.playlist_uid.as_str()).copied().unwrap_or(0);
                let theirs = they
                    .playlists
                    .get(m.playlist_uid.as_str())
                    .map(|p| p.updated_at)
                    .unwrap_or(0);
                if mine > theirs {
                    out.memberships.push(m.clone());
                }
            }
            _ => {}
        }
    }

    out.tombstones = local.tombstones.clone();

    // Scope: send whatever the other side is missing or has gone stale on.
    // It travels in both directions like any other replicated data — every
    // device can edit everyone's scope, including its own.
    for e in &local.scopes {
        let theirs = they
            .scopes
            .get(&(e.device_uid.as_str(), e.playlist_uid.as_str()));
        if theirs.map(|r| e.updated_at > r.updated_at).unwrap_or(true) {
            out.scopes.push(e.clone());
        }
    }
    for m in &local.device_sync {
        let theirs = they.device_sync.get(m.device_uid.as_str());
        if theirs.map(|r| m.updated_at > r.updated_at).unwrap_or(true) {
            out.device_sync.push(m.clone());
        }
    }
    out
}

fn has_tombstone(conn: &Connection, entity: &str, uid: &str) -> bool {
    tombstone_at(conn, entity, uid).is_some()
}

/// The tombstone's `deleted_at`, if there is one.
fn tombstone_at(conn: &Connection, entity: &str, uid: &str) -> Option<i64> {
    conn.query_row(
        "SELECT deleted_at FROM tombstones WHERE entity = ?1 AND uid = ?2",
        rusqlite::params![entity, uid],
        |r| r.get(0),
    )
    .optional()
    .ok()
    .flatten()
}

/// Applies the received changes, deletes included.
///
/// A delete is always applied. There used to be a per-device policy that
/// allowed ignoring or queuing them for confirmation, removed in Phase 6.4
/// because it couldn't deliver what it promised: it filtered by **who
/// passed you** the tombstone, not by who deleted. With three devices, a
/// delete this one rejected on the phone still got in through the laptop,
/// which had accepted it. With a server always on, this stopped being a
/// rare case and became the rule.
///
/// What protects against an accidental delete is the trash, which filters
/// nothing and always works: the file goes to `trash` and stays
/// there for 30 days.
///
/// `confirm_from`: the peer these changes come from, when this device has
/// someone to ask (the app; the server has no screen and passes `None`).
/// Its deletions from before the current pairing are held instead of
/// applied — see the module doc and `pending_deletes`.
pub fn apply(
    conn: &Connection,
    changes: &Changes,
    music_dir: &std::path::Path,
    confirm_from: Option<&str>,
) -> rusqlite::Result<Applied> {
    let mut applied = Applied::default();

    // --- Track metadata ------------------------------------------------
    for t in &changes.tracks {
        let local: Option<(i64, i64)> = conn
            .query_row(
                "SELECT id, updated_at FROM tracks WHERE uid = ?1",
                [&t.uid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        // A track that isn't here doesn't get created from loose metadata:
        // its row arrives together with the file (5.4). Otherwise there'd
        // be an entry left over that can't be played.
        let Some((id, local_updated)) = local else { continue };
        if t.updated_at <= local_updated {
            continue;
        }
        conn.execute(
            "UPDATE tracks SET title = ?1, artist = ?2, album = ?3, genre = ?4,
                    duration_ms = ?5, bpm = ?6, updated_at = ?7
             WHERE id = ?8",
            rusqlite::params![
                t.title,
                t.artist,
                t.album,
                t.genre,
                t.duration_ms,
                t.bpm,
                t.updated_at,
                id
            ],
        )?;
        applied.tracks += 1;
    }

    // --- Playlists and folders ------------------------------------------
    //
    // Two passes: first everything is created (with no parent), then it's
    // hooked up. They can arrive in any order and a folder can arrive
    // after its children; resolving the parent in a single pass would
    // leave dangling nodes.
    for p in &changes.playlists {
        if let Some(deleted_at) = tombstone_at(conn, "playlist", &p.uid) {
            if deleted_at >= p.updated_at {
                continue;
            }
            // Touched after it was deleted here (kept on the other side):
            // the delete is stale.
            conn.execute(
                "DELETE FROM tombstones WHERE entity = 'playlist' AND uid = ?1",
                [&p.uid],
            )?;
        }
        let existing: Option<(i64, i64)> = conn
            .query_row(
                "SELECT id, updated_at FROM playlists WHERE uid = ?1",
                [&p.uid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        match existing {
            Some((id, local_updated)) if p.updated_at > local_updated => {
                conn.execute(
                    "UPDATE playlists SET name = ?1, kind = ?2, rank = ?3, updated_at = ?4
                     WHERE id = ?5",
                    rusqlite::params![p.name, p.kind, p.rank, p.updated_at, id],
                )?;
                applied.playlists += 1;
            }
            Some(_) => {}
            None => {
                conn.execute(
                    "INSERT INTO playlists (uid, name, kind, parent_id, rank, updated_at)
                     VALUES (?1, ?2, ?3, NULL, ?4, ?5)",
                    rusqlite::params![p.uid, p.name, p.kind, p.rank, p.updated_at],
                )?;
                applied.playlists += 1;
            }
        }
    }
    // Second pass: now the parents actually exist.
    for p in &changes.playlists {
        if has_tombstone(conn, "playlist", &p.uid) {
            continue;
        }
        let parent_id: Option<i64> = match &p.parent_uid {
            Some(puid) => conn
                .query_row("SELECT id FROM playlists WHERE uid = ?1", [puid], |r| r.get(0))
                .optional()?,
            None => None,
        };
        conn.execute(
            "UPDATE playlists SET parent_id = ?1 WHERE uid = ?2",
            rusqlite::params![parent_id, p.uid],
        )?;
    }

    // --- Memberships -----------------------------------------------------
    for m in &changes.memberships {
        let key = format!("{}:{}", m.playlist_uid, m.track_uid);
        // Same as above: the local tombstone only wins if it's later than
        // the incoming add.
        if let Some(deleted_at) = tombstone_at(conn, "playlist_track", &key) {
            if deleted_at >= m.added_at {
                continue;
            }
            // The add is newer: the old delete is stale and in the way.
            conn.execute(
                "DELETE FROM tombstones WHERE entity = 'playlist_track' AND uid = ?1",
                [&key],
            )?;
        }
        let ids: Option<(i64, i64)> = conn
            .query_row(
                "SELECT p.id, t.id FROM playlists p, tracks t
                 WHERE p.uid = ?1 AND t.uid = ?2",
                rusqlite::params![m.playlist_uid, m.track_uid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        // If the track hasn't reached this device yet, there's no row to
        // link: the next sync brings the membership again, once the track
        // exists. Meanwhile it's parked, only so the playlist's count is
        // the real one (see `parked_memberships`).
        let Some((pid, tid)) = ids else {
            conn.execute(
                "INSERT INTO parked_memberships (playlist_uid, track_uid, added_at)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(playlist_uid, track_uid) DO UPDATE SET added_at = excluded.added_at
                 WHERE excluded.added_at > parked_memberships.added_at",
                rusqlite::params![m.playlist_uid, m.track_uid, m.added_at],
            )?;
            continue;
        };
        conn.execute(
            "DELETE FROM parked_memberships WHERE playlist_uid = ?1 AND track_uid = ?2",
            rusqlite::params![m.playlist_uid, m.track_uid],
        )?;
        // `DO UPDATE` and not `OR IGNORE`: a pair that already exists can
        // arrive with a different rank, which is how a reorder travels.
        let n = conn.execute(
            "INSERT INTO playlist_tracks (playlist_id, track_id, rank, added_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(playlist_id, track_id) DO UPDATE SET rank = excluded.rank
             WHERE playlist_tracks.rank <> excluded.rank",
            rusqlite::params![pid, tid, m.rank, m.added_at],
        )?;
        applied.memberships += n;
    }

    // --- Selective scope (Phase 5.7) --------------------------------------
    //
    // Replicated data like anything else, LWW per row: the phone's scope
    // gets edited from the PC and vice versa.
    for e in &changes.scopes {
        if crate::scope::apply_entry(conn, e)? {
            applied.scope += 1;
        }
    }
    for m in &changes.device_sync {
        if crate::scope::apply_device_sync(conn, m)? {
            applied.scope += 1;
        }
    }

    // --- Tombstones (Phase 5.6, 2.7) ----------------------------------------
    //
    // Saved when they apply, or when there's nothing here to delete: this is
    // what prevents this device from giving back to the other side what it
    // already deleted. One that loses to a newer item isn't saved (it would
    // travel on and lose everywhere else too); one that's held isn't saved
    // until the user confirms it.
    let linked_since = match confirm_from {
        Some(peer) => paired_at(conn, peer)?,
        None => None,
    };
    for t in &changes.tombstones {
        match target(conn, t)? {
            Target::Newer => continue,
            Target::Present => {
                if let (Some(peer), Some(since)) = (confirm_from, linked_since) {
                    if t.deleted_at < since {
                        conn.execute(
                            "INSERT OR REPLACE INTO held_deletes
                                 (entity, uid, deleted_at, from_device)
                             VALUES (?1, ?2, ?3, ?4)",
                            rusqlite::params![t.entity, t.uid, t.deleted_at, peer],
                        )?;
                        applied.held += 1;
                        continue;
                    }
                }
            }
            Target::Gone => {}
        }
        store_tombstone(conn, t)?;
        applied.deleted += apply_tombstone(conn, music_dir, t)?;
    }

    Ok(applied)
}

/// When this device paired with `peer` (the current pairing: re-linking
/// overwrites it). `None` if it isn't a known device.
fn paired_at(conn: &Connection, peer: &str) -> rusqlite::Result<Option<i64>> {
    Ok(conn
        .query_row("SELECT paired_at FROM devices WHERE uid = ?1", [peer], |r| {
            r.get::<_, Option<i64>>(0)
        })
        .optional()?
        .flatten())
}

/// What a tombstone would hit on this device.
enum Target {
    /// Nothing: not here, or already gone.
    Gone,
    /// Here, and not touched since the delete.
    Present,
    /// Here, and edited or re-added after the delete: it wins.
    Newer,
}

fn target(conn: &Connection, t: &Tombstone) -> rusqlite::Result<Target> {
    let stamp: Option<i64> = match t.entity.as_str() {
        "track" => conn
            .query_row("SELECT updated_at FROM tracks WHERE uid = ?1", [&t.uid], |r| r.get(0))
            .optional()?,
        "playlist" => conn
            .query_row("SELECT updated_at FROM playlists WHERE uid = ?1", [&t.uid], |r| r.get(0))
            .optional()?,
        "playlist_track" => {
            let Some((pl, tr)) = t.uid.split_once(':') else { return Ok(Target::Gone) };
            conn.query_row(
                "SELECT pt.added_at FROM playlist_tracks pt
                 JOIN playlists p ON p.id = pt.playlist_id
                 JOIN tracks t ON t.id = pt.track_id
                 WHERE p.uid = ?1 AND t.uid = ?2",
                rusqlite::params![pl, tr],
                |r| r.get(0),
            )
            .optional()?
        }
        _ => None,
    };
    Ok(match stamp {
        None => Target::Gone,
        Some(at) if at > t.deleted_at => Target::Newer,
        Some(_) => Target::Present,
    })
}

fn store_tombstone(conn: &Connection, t: &Tombstone) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO tombstones (entity, uid, deleted_at, device_uid)
         VALUES (?1, ?2, ?3, '')",
        rusqlite::params![t.entity, t.uid, t.deleted_at],
    )?;
    // Applied (by this sync or through another device): no longer a question.
    conn.execute(
        "DELETE FROM held_deletes WHERE entity = ?1 AND uid = ?2",
        rusqlite::params![t.entity, t.uid],
    )?;
    Ok(())
}

/// Deletions held for the user, per device that made them.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PendingDeletes {
    pub device_uid: String,
    pub device_name: String,
    pub tracks: usize,
    pub playlists: usize,
    /// Tracks taken out of a playlist (the track itself stays).
    pub removals: usize,
}

fn held(conn: &Connection, from: Option<&str>) -> rusqlite::Result<Vec<Tombstone>> {
    let mut stmt = conn.prepare(
        "SELECT entity, uid, deleted_at FROM held_deletes
         WHERE ?1 IS NULL OR from_device = ?1",
    )?;
    let rows = stmt.query_map([from], |r| {
        Ok(Tombstone {
            entity: r.get(0)?,
            uid: r.get(1)?,
            deleted_at: r.get(2)?,
        })
    })?;
    rows.collect()
}

/// Drops held deletions that are no longer a question: the item is gone
/// (deleted by hand meanwhile) or was touched after the delete.
fn prune_pending(conn: &Connection) -> rusqlite::Result<()> {
    for t in held(conn, None)? {
        if !matches!(target(conn, &t)?, Target::Present) {
            conn.execute(
                "DELETE FROM held_deletes WHERE entity = ?1 AND uid = ?2",
                rusqlite::params![t.entity, t.uid],
            )?;
        }
    }
    Ok(())
}

pub fn pending_deletes(conn: &Connection) -> rusqlite::Result<Vec<PendingDeletes>> {
    prune_pending(conn)?;
    let mut stmt = conn.prepare(
        "SELECT p.from_device, COALESCE(d.name, ''), p.entity, COUNT(*)
         FROM held_deletes p LEFT JOIN devices d ON d.uid = p.from_device
         GROUP BY p.from_device, p.entity
         ORDER BY p.from_device",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i64>(3)? as usize,
        ))
    })?;
    let mut out: Vec<PendingDeletes> = Vec::new();
    for row in rows {
        let (device, name, entity, n) = row?;
        if out.last().map(|g| g.device_uid != device).unwrap_or(true) {
            out.push(PendingDeletes {
                device_uid: device,
                device_name: name,
                ..Default::default()
            });
        }
        let g = out.last_mut().expect("pushed above");
        match entity.as_str() {
            "track" => g.tracks += n,
            "playlist" => g.playlists += n,
            "playlist_track" => g.removals += n,
            _ => {}
        }
    }
    Ok(out)
}

/// The user's answer for the deletions `device` made while unlinked.
///
/// - `keep = false`: delete here too. The tombstones are saved and applied
///   like any other (files to the trash).
/// - `keep = true`: the items get a fresh clock, so on the next sync they
///   beat the other side's tombstone and travel back to it — and to anyone
///   else that applied the delete.
///
/// Returns how many items it touched.
pub fn resolve_pending(
    conn: &Connection,
    music_dir: &std::path::Path,
    device: &str,
    keep: bool,
) -> rusqlite::Result<usize> {
    prune_pending(conn)?;
    let now = crate::db::now_ms();
    let mut n = 0;
    for t in held(conn, Some(device))? {
        if keep {
            n += match t.entity.as_str() {
                "track" => conn.execute(
                    "UPDATE tracks SET updated_at = ?1 WHERE uid = ?2",
                    rusqlite::params![now, t.uid],
                )?,
                "playlist" => conn.execute(
                    "UPDATE playlists SET updated_at = ?1 WHERE uid = ?2",
                    rusqlite::params![now, t.uid],
                )?,
                "playlist_track" => match t.uid.split_once(':') {
                    Some((pl, tr)) => conn.execute(
                        "UPDATE playlist_tracks SET added_at = ?1
                         WHERE playlist_id = (SELECT id FROM playlists WHERE uid = ?2)
                           AND track_id = (SELECT id FROM tracks WHERE uid = ?3)",
                        rusqlite::params![now, pl, tr],
                    )?,
                    None => 0,
                },
                _ => 0,
            };
        } else {
            store_tombstone(conn, &t)?;
            n += apply_tombstone(conn, music_dir, &t)?;
        }
        conn.execute(
            "DELETE FROM held_deletes WHERE entity = ?1 AND uid = ?2",
            rusqlite::params![t.entity, t.uid],
        )?;
    }
    Ok(n)
}

/// Applies a delete. Returns 1 if it deleted something, 0 if there was
/// nothing to delete.
///
/// The audio file **is not destroyed**: it goes to the library trash,
/// where it survives 30 days. A local delete you did while watching the
/// screen; one arriving over the network had better be recoverable.
fn apply_tombstone(
    conn: &Connection,
    music_dir: &std::path::Path,
    t: &Tombstone,
) -> rusqlite::Result<usize> {
    match t.entity.as_str() {
        "track" => {
            // A track that never got here can still be parked in playlists.
            conn.execute("DELETE FROM parked_memberships WHERE track_uid = ?1", [&t.uid])?;
            let row: Option<(i64, String)> = conn
                .query_row(
                    "SELECT id, path FROM tracks WHERE uid = ?1",
                    [&t.uid],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let Some((id, path)) = row else { return Ok(0) };
            let path = std::path::Path::new(&path);
            // Only what lives in the managed folder gets touched. A legacy
            // file from outside isn't ours to move.
            if path.starts_with(music_dir) && path.exists() {
                match crate::trash::move_to_trash(music_dir, path) {
                    Ok(dest) => log::info!("[sync] moved to trash: {}", dest.display()),
                    Err(e) => {
                        // If the file couldn't be moved, the row stays: a
                        // row with no file is worse than a delete that
                        // didn't apply and gets retried on the next sync.
                        log::warn!("[sync] could not move {} to trash: {e}", path.display());
                        return Ok(0);
                    }
                }
            }
            // The CASCADE removes it from every playlist.
            conn.execute("DELETE FROM tracks WHERE id = ?1", [id])?;
            Ok(1)
        }
        "playlist" => {
            conn.execute("DELETE FROM parked_memberships WHERE playlist_uid = ?1", [&t.uid])?;
            let n = conn.execute("DELETE FROM playlists WHERE uid = ?1", [&t.uid])?;
            Ok(n)
        }
        "playlist_track" => {
            let Some((pl, tr)) = t.uid.split_once(':') else { return Ok(0) };
            conn.execute(
                "DELETE FROM parked_memberships
                 WHERE playlist_uid = ?1 AND track_uid = ?2 AND added_at <= ?3",
                rusqlite::params![pl, tr, t.deleted_at],
            )?;
            // Only if the delete is later than the local add. If it was
            // added again after that, that add is the last word.
            let n = conn.execute(
                "DELETE FROM playlist_tracks
                 WHERE playlist_id = (SELECT id FROM playlists WHERE uid = ?1)
                   AND track_id = (SELECT id FROM tracks WHERE uid = ?2)
                   AND added_at <= ?3",
                rusqlite::params![pl, tr, t.deleted_at],
            )?;
            Ok(n)
        }
        _ => Ok(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn mem() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        db::init_schema(&conn).unwrap();
        conn
    }

    fn add_track(conn: &Connection, uid: &str, title: &str, updated_at: i64) -> i64 {
        conn.execute(
            "INSERT INTO tracks (path, uid, title, updated_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![format!("/m/{uid}"), uid, title, updated_at],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn entry(uid: &str, title: &str, updated_at: i64) -> TrackEntry {
        TrackEntry {
            uid: uid.into(),
            hash: Some("h".into()),
            size: 1,
            filename: format!("{uid}.flac"),
            title: title.into(),
            artist: "A".into(),
            album: String::new(),
            genre: String::new(),
            duration_ms: 0,
            bpm: None,
            updated_at,
            present: true,
        }
    }

    fn pl(uid: &str, name: &str, parent: Option<&str>, updated_at: i64) -> PlaylistEntry {
        PlaylistEntry {
            uid: uid.into(),
            name: name.into(),
            kind: if parent.is_none() && uid.starts_with("f") { "folder".into() } else { "playlist".into() },
            parent_uid: parent.map(|s| s.to_string()),
            rank: "V".into(),
            updated_at,
            held: true,
        }
    }

    #[test]
    fn newer_metadata_wins_and_older_is_ignored() {
        let conn = mem();
        add_track(&conn, "t1", "old", 100);
        let changes = Changes {
            tracks: vec![entry("t1", "new", 500)],
            ..Default::default()
        };
        assert_eq!(apply(&conn, &changes, std::path::Path::new("/nowhere"), None).unwrap().tracks, 1);
        let title: String = conn
            .query_row("SELECT title FROM tracks WHERE uid = 't1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(title, "new");

        // An older change overwrites nothing.
        let old = Changes {
            tracks: vec![entry("t1", "previous", 200)],
            ..Default::default()
        };
        assert_eq!(apply(&conn, &old, std::path::Path::new("/nowhere"), None).unwrap().tracks, 0);
        let title: String = conn
            .query_row("SELECT title FROM tracks WHERE uid = 't1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(title, "new");
    }

    /// Applying the same thing twice can't change anything the second time:
    /// otherwise every sync would count as pending work forever.
    #[test]
    fn applying_twice_is_a_no_op() {
        let conn = mem();
        add_track(&conn, "t1", "old", 100);
        let changes = Changes {
            tracks: vec![entry("t1", "new", 500)],
            playlists: vec![pl("p1", "Sets", None, 10)],
            memberships: vec![Membership {
                playlist_uid: "p1".into(),
                track_uid: "t1".into(),
                rank: "V".into(),
                added_at: 0,
            }],
            ..Default::default()
        };
        let first = apply(&conn, &changes, std::path::Path::new("/nowhere"), None).unwrap();
        assert!(first.total() > 0);
        let second = apply(&conn, &changes, std::path::Path::new("/nowhere"), None).unwrap();
        assert_eq!(second.total(), 0, "the second pass must not change anything");
    }

    /// Folders can arrive after their children: the hierarchy still has to
    /// come out right.
    #[test]
    fn hierarchy_resolves_regardless_of_arrival_order() {
        let conn = mem();
        let changes = Changes {
            // The child first, the parent after.
            playlists: vec![
                pl("p1", "Warmup", Some("f1"), 10),
                pl("f1", "Electronica", None, 10),
            ],
            ..Default::default()
        };
        apply(&conn, &changes, std::path::Path::new("/nowhere"), None).unwrap();

        let (child_parent, folder_id): (Option<i64>, i64) = (
            conn.query_row("SELECT parent_id FROM playlists WHERE uid = 'p1'", [], |r| r.get(0))
                .unwrap(),
            conn.query_row("SELECT id FROM playlists WHERE uid = 'f1'", [], |r| r.get(0))
                .unwrap(),
        );
        assert_eq!(child_parent, Some(folder_id));
    }

    /// A playlist deleted here doesn't come back just because the other
    /// side still has it.
    #[test]
    fn tombstoned_playlists_are_not_recreated() {
        let conn = mem();
        conn.execute(
            "INSERT INTO tombstones (entity, uid, deleted_at) VALUES ('playlist', 'p1', 999)",
            [],
        )
        .unwrap();
        let changes = Changes {
            playlists: vec![pl("p1", "Sets", None, 10)],
            ..Default::default()
        };
        assert_eq!(apply(&conn, &changes, std::path::Path::new("/nowhere"), None).unwrap().playlists, 0);
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM playlists", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }

    /// Removing a track from a playlist only propagates with a tombstone;
    /// without one, the union puts it back (adding beats a concurrent
    /// removal).
    #[test]
    fn removed_membership_stays_removed_only_with_a_tombstone() {
        let conn = mem();
        add_track(&conn, "t1", "x", 1);
        apply(
            &conn,
            &Changes {
                playlists: vec![pl("p1", "Sets", None, 10)],
                ..Default::default()
            },
            std::path::Path::new("/nowhere"), None)
        .unwrap();
        let member = Changes {
            memberships: vec![Membership {
                playlist_uid: "p1".into(),
                track_uid: "t1".into(),
                rank: "V".into(),
                added_at: 0,
            }],
            ..Default::default()
        };
        assert_eq!(apply(&conn, &member, std::path::Path::new("/nowhere"), None).unwrap().memberships, 1);

        // Removed here, with a record of it.
        conn.execute("DELETE FROM playlist_tracks", []).unwrap();
        conn.execute(
            "INSERT INTO tombstones (entity, uid, deleted_at) VALUES ('playlist_track','p1:t1',999)",
            [],
        )
        .unwrap();
        assert_eq!(apply(&conn, &member, std::path::Path::new("/nowhere"), None).unwrap().memberships, 0, "must not come back");
    }

    /// A membership for a track that hasn't arrived yet is skipped without
    /// breaking anything; the next sync brings it once the file is there.
    #[test]
    fn membership_of_an_unknown_track_is_skipped() {
        let conn = mem();
        let changes = Changes {
            playlists: vec![pl("p1", "Sets", None, 10)],
            memberships: vec![Membership {
                playlist_uid: "p1".into(),
                track_uid: "not-yet".into(),
                rank: "V".into(),
                added_at: 0,
            }],
            ..Default::default()
        };
        let applied = apply(&conn, &changes, std::path::Path::new("/nowhere"), None).unwrap();
        assert_eq!(applied.playlists, 1);
        assert_eq!(applied.memberships, 0);
    }

    fn count_of(conn: &Connection, uid: &str) -> i64 {
        db::list_playlists(conn)
            .unwrap()
            .into_iter()
            .find(|n| n.uid.as_deref() == Some(uid))
            .unwrap()
            .track_count
    }

    /// A device that syncs only some playlists still shows how many tracks
    /// the others have (1.8): the memberships it can't link are parked and
    /// counted, a removal clears them, and resending doesn't double them.
    #[test]
    fn parked_memberships_give_the_real_count_until_removed() {
        let conn = mem();
        let member = |track: &str| Membership {
            playlist_uid: "p1".into(),
            track_uid: track.into(),
            rank: "V".into(),
            added_at: 5,
        };
        let changes = Changes {
            playlists: vec![pl("p1", "Sets", None, 10)],
            memberships: vec![member("a"), member("b")],
            ..Default::default()
        };
        let dir = std::path::Path::new("/nowhere");
        apply(&conn, &changes, dir, None).unwrap();
        assert_eq!(count_of(&conn, "p1"), 2);
        // Every sync resends them: still two.
        apply(&conn, &changes, dir, None).unwrap();
        assert_eq!(count_of(&conn, "p1"), 2);

        // Removed from the playlist on the other side.
        let removal = Changes {
            tombstones: vec![tomb("playlist_track", "p1:a")],
            ..Default::default()
        };
        apply(&conn, &removal, dir, None).unwrap();
        assert_eq!(count_of(&conn, "p1"), 1);

        // Deleted everywhere.
        let gone = Changes {
            tombstones: vec![tomb("track", "b")],
            ..Default::default()
        };
        apply(&conn, &gone, dir, None).unwrap();
        assert_eq!(count_of(&conn, "p1"), 0);
    }

    /// Metadata for a track whose file isn't there doesn't create a ghost
    /// entry: the row arrives together with the file.
    #[test]
    fn metadata_for_an_unknown_track_does_not_create_a_row() {
        let conn = mem();
        let changes = Changes {
            tracks: vec![entry("never-seen", "x", 10)],
            ..Default::default()
        };
        assert_eq!(apply(&conn, &changes, std::path::Path::new("/nowhere"), None).unwrap().tracks, 0);
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }

    fn tmp_music(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "sway-merge-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn add_track_at(conn: &Connection, uid: &str, path: &std::path::Path) {
        conn.execute(
            "INSERT INTO tracks (path, uid, title, updated_at) VALUES (?1, ?2, 'x', 1)",
            rusqlite::params![path.to_string_lossy(), uid],
        )
        .unwrap();
    }

    fn tomb(entity: &str, uid: &str) -> Tombstone {
        Tombstone {
            entity: entity.into(),
            uid: uid.into(),
            deleted_at: 1000,
        }
    }

    /// A delete that arrives over the network removes the track from the
    /// library, but the file goes to the trash: recoverable, not destroyed.
    #[test]
    fn a_propagated_delete_moves_the_file_to_the_trash() {
        let music = tmp_music("del");
        let conn = mem();
        let f = music.join("deleted.flac");
        std::fs::write(&f, b"audio").unwrap();
        add_track_at(&conn, "t1", &f);

        let changes = Changes {
            tombstones: vec![tomb("track", "t1")],
            ..Default::default()
        };
        let applied = apply(&conn, &changes, &music, None).unwrap();

        assert_eq!(applied.deleted, 1);
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "leaves the library");
        assert!(!f.exists(), "and the managed folder");
        let recoverable = std::fs::read_dir(crate::trash::trash_dir(&music))
            .unwrap()
            .flatten()
            .any(|e| std::fs::read(e.path()).unwrap() == b"audio");
        assert!(recoverable, "but still exists in the trash");
        std::fs::remove_dir_all(&music).ok();
    }

    /// Scope travels like any other replicated data: I edit it here for
    /// the phone and the phone finds out.
    #[test]
    fn scope_rows_travel_and_the_newest_wins() {
        let mut local = empty_manifest();
        local.scopes.push(ScopeEntry {
            device_uid: "phone".into(),
            playlist_uid: "sets".into(),
            selected: true,
            updated_at: 500,
        });
        local.device_sync.push(DeviceSync {
            device_uid: "phone".into(),
            mode: "selected".into(),
            direction: "both".into(),
            updated_at: 500,
        });
        let mut remote = empty_manifest();
        remote.scopes.push(ScopeEntry {
            device_uid: "phone".into(),
            playlist_uid: "sets".into(),
            selected: false,
            updated_at: 100,
        });

        let out = changes_for_peer(&local, &remote);
        assert_eq!(out.scopes.len(), 1, "mine is newer: it travels");
        assert_eq!(out.device_sync.len(), 1);

        // And nothing comes back the other way: theirs went stale.
        assert!(changes_for_peer(&remote, &local).scopes.is_empty());

        let conn = mem();
        let applied = apply(&conn, &out, std::path::Path::new("/nowhere"), None).unwrap();
        assert_eq!(applied.scope, 2);
        let s = crate::scope::get(&conn, "phone").unwrap();
        assert_eq!(s.mode, crate::scope::Mode::Selected);
        assert!(s.selected.contains("sets"));
    }

    /// Deleting a playlist can't take its music with it.
    #[test]
    fn deleting_a_playlist_does_not_delete_its_tracks() {
        let music = tmp_music("pl");
        let conn = mem();
        let f = music.join("survives.flac");
        std::fs::write(&f, b"audio").unwrap();
        add_track_at(&conn, "t1", &f);
        apply(
            &conn,
            &Changes {
                playlists: vec![pl("p1", "Sets", None, 10)],
                memberships: vec![Membership {
                    playlist_uid: "p1".into(),
                    track_uid: "t1".into(),
                    rank: "V".into(),
                added_at: 0,
                }],
                ..Default::default()
            },
            &music, None)
        .unwrap();

        apply(
            &conn,
            &Changes {
                tombstones: vec![tomb("playlist", "p1")],
                ..Default::default()
            },
            &music, None)
        .unwrap();

        let playlists: i64 = conn
            .query_row("SELECT COUNT(*) FROM playlists", [], |r| r.get(0))
            .unwrap();
        let tracks: i64 = conn
            .query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(playlists, 0);
        assert_eq!(tracks, 1, "the track stays in the library");
        assert!(f.exists(), "and so does its file");
        std::fs::remove_dir_all(&music).ok();
    }

    /// A file outside the managed folder (legacy) isn't ours to move: it's
    /// removed from the library and the file stays where it is.
    #[test]
    fn a_file_outside_the_managed_folder_is_left_alone() {
        let music = tmp_music("legacy-managed");
        let elsewhere = tmp_music("legacy-outside");
        let conn = mem();
        let f = elsewhere.join("foreign.flac");
        std::fs::write(&f, b"audio").unwrap();
        add_track_at(&conn, "t1", &f);

        apply(
            &conn,
            &Changes {
                tombstones: vec![tomb("track", "t1")],
                ..Default::default()
            },
            &music, None)
        .unwrap();

        assert!(f.exists(), "the foreign file isn't touched");
        std::fs::remove_dir_all(&music).ok();
        std::fs::remove_dir_all(&elsewhere).ok();
    }

    fn manifest_with(playlist_updated: i64, rank: &str) -> Manifest {
        Manifest {
            device_uid: "d".into(),
            tracks: vec![entry("t1", "x", 1)],
            playlists: vec![pl("p1", "Sets", None, playlist_updated)],
            memberships: vec![Membership {
                playlist_uid: "p1".into(),
                track_uid: "t1".into(),
                rank: rank.into(),
                added_at: 1,
            }],
            tombstones: vec![],
            ..Default::default()
        }
    }

    /// Reordering neither adds nor removes anything: the pair already
    /// exists on both sides and only its rank changes. Without this,
    /// moving a song within a playlist never propagated — and the order of
    /// a set is kind of the point.
    #[test]
    fn reordering_travels_when_the_playlist_is_newer() {
        let reordered = manifest_with(500, "b");
        let stale = manifest_with(100, "a");

        let c = changes_for_peer(&reordered, &stale);
        assert_eq!(c.memberships.len(), 1, "the new rank has to travel");
        assert_eq!(c.memberships[0].rank, "b");

        // And not in the other direction: whoever touched it last wins.
        let c = changes_for_peer(&stale, &reordered);
        assert!(c.memberships.is_empty());
    }

    /// Same order on both sides: nothing to send, no matter how different
    /// the playlist's dates are.
    #[test]
    fn an_unchanged_order_is_not_resent() {
        let a = manifest_with(500, "a");
        let b = manifest_with(100, "a");
        assert!(changes_for_peer(&a, &b).memberships.is_empty());
    }

    /// And when applying, a pair that already exists has to end up with the
    /// new rank.
    #[test]
    fn applying_a_reorder_updates_the_existing_rank() {
        let conn = mem();
        add_track(&conn, "t1", "x", 1);
        let base = Changes {
            playlists: vec![pl("p1", "Sets", None, 10)],
            memberships: vec![Membership {
                playlist_uid: "p1".into(),
                track_uid: "t1".into(),
                rank: "a".into(),
                added_at: 0,
            }],
            ..Default::default()
        };
        let music = std::path::Path::new("/nowhere");
        apply(&conn, &base, music, None).unwrap();

        let reorder = Changes {
            memberships: vec![Membership {
                playlist_uid: "p1".into(),
                track_uid: "t1".into(),
                rank: "z".into(),
                added_at: 0,
            }],
            ..Default::default()
        };
        assert_eq!(
            apply(&conn, &reorder, music, None)
                .unwrap()
                .memberships,
            1
        );
        let rank: String = conn
            .query_row("SELECT rank FROM playlist_tracks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rank, "z");

        // And applying it again doesn't count as work.
        assert_eq!(
            apply(&conn, &reorder, music, None)
                .unwrap()
                .memberships,
            0
        );
    }

    fn empty_manifest() -> Manifest {
        Manifest {
            device_uid: "d".into(),
            tracks: vec![],
            playlists: vec![],
            memberships: vec![],
            tombstones: vec![],
            ..Default::default()
        }
    }

    fn member(rank: &str, added_at: i64) -> Membership {
        Membership {
            playlist_uid: "p1".into(),
            track_uid: "t1".into(),
            rank: rank.into(),
            added_at,
        }
    }

    /// Re-adding a song to a playlist it had been removed from. The
    /// tombstone from back then can't beat the new add: if it did, adding
    /// it back would undo itself on the next sync — which is exactly what
    /// used to happen.
    #[test]
    fn re_adding_a_track_beats_an_older_removal() {
        let music = std::path::Path::new("/nowhere");
        let conn = mem();
        add_track(&conn, "t1", "x", 1);
        apply(
            &conn,
            &Changes {
                playlists: vec![pl("p1", "Sets", None, 10)],
                ..Default::default()
            },
            music, None)
        .unwrap();

        // Removed a while ago...
        conn.execute(
            "INSERT INTO tombstones (entity, uid, deleted_at) VALUES ('playlist_track','p1:t1',100)",
            [],
        )
        .unwrap();

        // ...and now a LATER add arrives.
        let re_add = Changes {
            memberships: vec![member("V", 500)],
            ..Default::default()
        };
        assert_eq!(apply(&conn, &re_add, music, None).unwrap().memberships, 1);

        // And the old tombstone gets cleared, so it doesn't get in the way again.
        assert!(!has_tombstone(&conn, "playlist_track", "p1:t1"));

        // A LATER tombstone does win.
        let removal = Changes {
            tombstones: vec![Tombstone {
                entity: "playlist_track".into(),
                uid: "p1:t1".into(),
                deleted_at: 900,
            }],
            ..Default::default()
        };
        assert_eq!(apply(&conn, &removal, music, None).unwrap().deleted, 1);
    }

    /// And a delete that arrives stale can't remove something added later.
    #[test]
    fn an_older_removal_does_not_undo_a_newer_add() {
        let music = std::path::Path::new("/nowhere");
        let conn = mem();
        add_track(&conn, "t1", "x", 1);
        apply(
            &conn,
            &Changes {
                playlists: vec![pl("p1", "Sets", None, 10)],
                memberships: vec![member("V", 800)],
                ..Default::default()
            },
            music, None)
        .unwrap();

        let stale = Changes {
            tombstones: vec![Tombstone {
                entity: "playlist_track".into(),
                uid: "p1:t1".into(),
                deleted_at: 200,
            }],
            ..Default::default()
        };
        assert_eq!(apply(&conn, &stale, music, None).unwrap().deleted, 0);
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM playlist_tracks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "the song stays");
    }

    /// The same criterion on the side deciding what to send: a new add
    /// isn't skipped because of an older tombstone from the other side.
    #[test]
    fn a_newer_add_is_still_sent_despite_an_older_remote_tombstone() {
        let mut local = empty_manifest();
        local.playlists.push(pl("p1", "Sets", None, 10));
        local.memberships.push(member("V", 500));

        let mut remote = empty_manifest();
        remote.playlists.push(pl("p1", "Sets", None, 10));
        remote.tombstones.push(Tombstone {
            entity: "playlist_track".into(),
            uid: "p1:t1".into(),
            deleted_at: 100,
        });

        assert_eq!(changes_for_peer(&local, &remote).memberships.len(), 1);

        // But if the other side's tombstone is later, it isn't sent.
        remote.tombstones[0].deleted_at = 900;
        assert!(changes_for_peer(&local, &remote).memberships.is_empty());
    }

    #[test]
    fn changes_for_peer_only_includes_what_is_newer_or_missing() {
        let local = Manifest {
            device_uid: "a".into(),
            tracks: vec![entry("t1", "new", 500), entry("t2", "same", 100)],
            playlists: vec![pl("p1", "Sets", None, 50)],
            memberships: vec![Membership {
                playlist_uid: "p1".into(),
                track_uid: "t1".into(),
                rank: "V".into(),
                added_at: 0,
            }],
            tombstones: vec![],
            ..Default::default()
        };
        let remote = Manifest {
            device_uid: "b".into(),
            tracks: vec![entry("t1", "old", 100), entry("t2", "same", 100)],
            playlists: vec![],
            memberships: vec![],
            tombstones: vec![],
            ..Default::default()
        };
        let c = changes_for_peer(&local, &remote);
        assert_eq!(c.tracks.len(), 1, "only t1, which is newer");
        assert_eq!(c.tracks[0].uid, "t1");
        assert_eq!(c.playlists.len(), 1);
        assert_eq!(c.memberships.len(), 1);
    }
}
