//! Sync progress outside the webview: the throttled `sync-progress` event and,
//! on Android, the notification of the `dataSync` foreground service that
//! keeps a sync alive with the screen off (Fase 1.2 / 1.3).
//!
//! Both directions feed it: the side that runs a sync (`engine::sync`) and
//! the side serving one (`engine::serve_requests`, e.g. the PC pushing files
//! to the phone). Several runs can be in flight at once, one per peer; the
//! notification shows the one that moved last and goes away with the last.
//!
//! The loudness sweep (`loudness.rs`: new tracks, or "Rescan") shares the same
//! notification and service — it's long work too, and the process has to
//! survive it. A sync wins while both exist (the sweep waits for syncs
//! anyway); when the last sync ends the notification goes back to the sweep.

use trak_core::engine::Progress;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

/// `sync-progress` used to go out per 1 MiB chunk, and the Sync panel
/// re-rendered on each one: part of the lag while syncing (1.2). A new file
/// and the end of one always go out.
const EVENT_EVERY: Duration = Duration::from_millis(200);
/// The notification is a round trip to the UI thread: once a second is plenty.
const NOTICE_EVERY: Duration = Duration::from_secs(1);

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SyncProgressEvent {
    uid: String,
    /// Index of the current file and total files for this run.
    file_index: usize,
    file_total: usize,
    filename: String,
    /// Bytes of the current file.
    done: u64,
    total: u64,
    sending: bool,
}

struct Run {
    name: String,
    last_event: Instant,
    last_index: usize,
}

#[derive(Default)]
pub struct SyncNotice {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    runs: HashMap<String, Run>,
    /// Loudness sweep in progress: (measured, total of this sweep).
    analysis: Option<(u32, u32)>,
    last_notice: Option<Instant>,
    shown: bool,
}

/// One chunk moved. Called from the sync threads, never the main thread.
pub fn progress(handle: &AppHandle, p: &Progress) {
    let state = handle.state::<crate::AppState>();
    let now = Instant::now();
    let (emit, notice) = {
        let Ok(mut inner) = state.sync_notice.inner.lock() else { return };
        let new_run = !inner.runs.contains_key(p.peer_uid);
        if new_run {
            let name = crate::pairing::peer_name(handle, p.peer_uid);
            inner.runs.insert(
                p.peer_uid.to_string(),
                Run { name, last_event: now - EVENT_EVERY, last_index: 0 },
            );
        }
        let run = inner.runs.get_mut(p.peer_uid).expect("inserted above");
        let file_edge = p.index != run.last_index || (p.total > 0 && p.done >= p.total);
        let emit = file_edge || now.duration_since(run.last_event) >= EVENT_EVERY;
        if emit {
            run.last_event = now;
            run.last_index = p.index;
        }
        let name = run.name.clone();
        let notice_due = !inner.shown
            || inner.last_notice.map_or(true, |t| now.duration_since(t) >= NOTICE_EVERY);
        let notice = notice_due.then(|| {
            inner.shown = true;
            inner.last_notice = Some(now);
            name
        });
        (emit, notice)
    };
    if emit {
        let _ = handle.emit(
            "sync-progress",
            SyncProgressEvent {
                uid: p.peer_uid.to_string(),
                file_index: p.index,
                file_total: p.total_files,
                filename: p.filename.to_string(),
                done: p.done,
                total: p.total,
                sending: p.sending,
            },
        );
    }
    // Outside the lock: it waits on the UI thread.
    if let Some(name) = notice {
        show(handle, &name, p);
    }
}

/// Whether files are moving right now, in either direction.
pub fn busy(handle: &AppHandle) -> bool {
    let state = handle.state::<crate::AppState>();
    let busy = state.sync_notice.inner.lock().map(|i| !i.runs.is_empty()).unwrap_or(false);
    busy
}

/// A run with `uid` ended (either side). Removes the notification when it
/// was the last one. `sync-idle` tells the toast: `sync-done` only exists on
/// the side that ran the sync, not on the one that served it.
pub fn end(handle: &AppHandle, uid: &str) {
    let _ = handle.emit("sync-idle", uid);
    let state = handle.state::<crate::AppState>();
    let next = {
        let Ok(mut inner) = state.sync_notice.inner.lock() else { return };
        inner.runs.remove(uid);
        if !inner.runs.is_empty() || !inner.shown {
            return;
        }
        match inner.analysis {
            // The sweep was waiting behind the sync: it's its turn now.
            Some(a) => {
                inner.last_notice = Some(Instant::now());
                Some(a)
            }
            None => {
                inner.shown = false;
                inner.last_notice = None;
                None
            }
        }
    };
    match next {
        Some((done, total)) => show_analysis(handle, done, total),
        None => hide(handle),
    }
}

/// The loudness sweep measured `done` of `total`. Throttled like the sync;
/// while a sync is in flight it's only recorded.
pub fn analysis(handle: &AppHandle, done: u32, total: u32) {
    let state = handle.state::<crate::AppState>();
    let now = Instant::now();
    let due = {
        let Ok(mut inner) = state.sync_notice.inner.lock() else { return };
        inner.analysis = Some((done, total));
        let due = inner.runs.is_empty()
            && (!inner.shown
                || inner.last_notice.map_or(true, |t| now.duration_since(t) >= NOTICE_EVERY));
        if due {
            inner.shown = true;
            inner.last_notice = Some(now);
        }
        due
    };
    if due {
        show_analysis(handle, done, total);
    }
}

/// The sweep finished (or gave up).
pub fn analysis_end(handle: &AppHandle) {
    let state = handle.state::<crate::AppState>();
    let hide_now = {
        let Ok(mut inner) = state.sync_notice.inner.lock() else { return };
        inner.analysis = None;
        let hide_now = inner.runs.is_empty() && inner.shown;
        if hide_now {
            inner.shown = false;
            inner.last_notice = None;
        }
        hide_now
    };
    if hide_now {
        hide(handle);
    }
}

fn hide(_handle: &AppHandle) {
    #[cfg(target_os = "android")]
    tauri_plugin_native_audio::hide_sync_notice(_handle);
}

#[cfg(not(target_os = "android"))]
fn show_analysis(_handle: &AppHandle, _done: u32, _total: u32) {}

#[cfg(target_os = "android")]
fn show_analysis(handle: &AppHandle, done: u32, total: u32) {
    let notice = tauri_plugin_native_audio::SyncNotice {
        title: "Analyzing tracks".to_string(),
        text: format!("{done} of {total} · loudness and silent edges"),
        done,
        total,
        icon: "analyze".to_string(),
    };
    tauri_plugin_native_audio::show_sync_notice(handle, &notice);
}

/// Only Android has the notification (and the service behind it).
#[cfg(not(target_os = "android"))]
fn show(_handle: &AppHandle, _name: &str, _p: &Progress) {}

#[cfg(target_os = "android")]
fn show(handle: &AppHandle, name: &str, p: &Progress) {
    let title = if p.sending {
        format!("Sending to {name}")
    } else {
        format!("Receiving from {name}")
    };
    let text = format!("{} of {} · {}", p.index, p.total_files, p.filename);
    let notice = tauri_plugin_native_audio::SyncNotice {
        title,
        text,
        done: p.index.saturating_sub(1) as u32,
        total: p.total_files as u32,
        icon: if p.sending { "upload" } else { "download" }.to_string(),
    };
    tauri_plugin_native_audio::show_sync_notice(handle, &notice);
}
