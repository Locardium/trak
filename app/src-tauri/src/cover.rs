// Cover art embedded in tags: extracted with lofty, downscaled to a thumb and
// kept on disk, one file per content hash, so each cover is decoded once per
// device instead of once per app start and per scroll (Fase 4.2).
//
// Before, thumbs lived only in memory and were made the first time a row
// scrolled into view: every playlist had to be opened once per session before
// its covers showed up quickly. Now a background pass (`Covers::warm`, kicked
// with the loudness sweep) makes the missing ones as tracks arrive.
use crate::id3_sanitize::sanitize_id3v2_date_frames;
use base64::Engine;
use lofty::file::{TaggedFile, TaggedFileExt};
use lofty::probe::Probe;
use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager};

const THUMB_SIZE: u32 = 256;

fn read_tagged(path: &Path) -> Option<TaggedFile> {
    match lofty::read_from_path(path) {
        Ok(t) => Some(t),
        Err(e) => {
            // Same case as import.rs::read_meta: a corrupt ID3 date frame
            // takes down the whole tag. Sanitize and retry in memory before
            // giving up (see id3_sanitize.rs).
            log::warn!("cover: lofty read_from_path failed for {}: {e}", path.display());
            let bytes = std::fs::read(path).ok()?;
            let patched = sanitize_id3v2_date_frames(&bytes)?;
            Probe::new(Cursor::new(patched.as_slice())).guess_file_type().ok()?.read().ok()
        }
    }
}

/// The embedded picture as a JPEG thumb, or `None` if the file has none.
fn thumb_jpeg(path: &Path) -> Option<Vec<u8>> {
    let tagged = read_tagged(path)?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag())?;
    let pic = tag.pictures().first()?;
    let img = image::load_from_memory(pic.data()).ok()?;
    let thumb = img.thumbnail(THUMB_SIZE, THUMB_SIZE).to_rgb8();
    let mut buf = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 82)
        .encode_image(&thumb)
        .ok()?;
    Some(buf)
}

fn data_url(jpeg: &[u8]) -> String {
    format!(
        "data:image/jpeg;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(jpeg)
    )
}

/// Thumbs by content hash: `<hash>.jpg`, or an empty `<hash>.none` for a file
/// without a cover (so it isn't decoded again every time to find nothing).
/// Keyed by hash and not by id or path: the same bytes are the same cover,
/// whatever the track is called or wherever it moved.
pub struct Covers {
    dir: PathBuf,
    /// Data URLs already handed out this session, by track id.
    memory: Mutex<HashMap<i64, Option<String>>>,
    warming: Arc<AtomicBool>,
}

impl Covers {
    pub fn new(dir: PathBuf) -> Self {
        if let Err(e) = std::fs::create_dir_all(&dir) {
            log::warn!("[cover] could not create {}: {e}", dir.display());
        }
        Self { dir, memory: Mutex::new(HashMap::new()), warming: Arc::default() }
    }

    /// The thumb for a track, as a data URL. Blocks on file I/O: never call it
    /// with a DB lock held.
    pub fn get(&self, id: i64, path: &Path, hash: Option<&str>) -> Option<String> {
        if let Some(c) = self.memory.lock().unwrap().get(&id) {
            return c.clone();
        }
        let jpeg = match hash {
            Some(h) => self.load_or_make(h, path),
            // Not hashed yet (backfill pending): no key to store it under.
            None => thumb_jpeg(path),
        };
        let url = jpeg.as_deref().map(data_url);
        self.memory.lock().unwrap().insert(id, url.clone());
        url
    }

    /// The thumb as a file, for whoever reads covers off disk (Android's
    /// notification). `make`: generate it if it isn't cached yet.
    pub fn file(&self, hash: &str, path: &Path, make: bool) -> Option<PathBuf> {
        let jpg = self.dir.join(format!("{hash}.jpg"));
        if !jpg.exists() && make {
            self.load_or_make(hash, path);
        }
        jpg.exists().then_some(jpg)
    }

    fn has(&self, hash: &str) -> bool {
        self.dir.join(format!("{hash}.jpg")).exists() || self.dir.join(format!("{hash}.none")).exists()
    }

    fn load_or_make(&self, hash: &str, path: &Path) -> Option<Vec<u8>> {
        let jpg = self.dir.join(format!("{hash}.jpg"));
        if let Ok(bytes) = std::fs::read(&jpg) {
            return Some(bytes);
        }
        let none = self.dir.join(format!("{hash}.none"));
        if none.exists() {
            return None;
        }
        let made = thumb_jpeg(path);
        // Written under a temporary name and renamed: a half-written JPEG
        // from a killed process would otherwise be served forever.
        let (dest, bytes): (&Path, &[u8]) = match &made {
            Some(b) => (&jpg, b),
            None => (&none, &[]),
        };
        let tmp = dest.with_extension("tmp");
        if let Err(e) = std::fs::write(&tmp, bytes).and_then(|_| std::fs::rename(&tmp, dest)) {
            log::warn!("[cover] could not store {}: {e}", dest.display());
        }
        made
    }

    /// Makes every missing thumb, on a background thread. Returns right away;
    /// a second call while one is running is a no-op (the running one reads
    /// the library fresh, so it reaches new tracks anyway on the next kick).
    pub fn warm(&self, app: AppHandle) {
        if self.warming.swap(true, Ordering::SeqCst) {
            return;
        }
        let warming = self.warming.clone();
        std::thread::spawn(move || {
            let t0 = std::time::Instant::now();
            let made = warm_missing(&app);
            if made > 0 {
                log::info!("[cover] made {made} thumb(s) in {:?}", t0.elapsed());
            }
            warming.store(false, Ordering::SeqCst);
        });
    }
}

fn warm_missing(app: &AppHandle) -> usize {
    let state = app.state::<crate::AppState>();
    let tracks: Vec<(String, String)> = {
        let Ok(conn) = state.db_read.lock() else { return 0 };
        let rows = conn
            .prepare(
                "SELECT content_hash, path FROM tracks
                 WHERE local_state = 'present' AND content_hash IS NOT NULL",
            )
            .and_then(|mut stmt| {
                stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<Vec<_>, _>>()
            });
        match rows {
            Ok(v) => v,
            Err(e) => {
                log::warn!("[cover] could not list tracks: {e}");
                return 0;
            }
        }
    };
    let mut made = 0;
    for (hash, path) in tracks {
        if state.covers.has(&hash) {
            continue;
        }
        // Same rule as the loudness sweep: files moving between devices
        // come first.
        while crate::sync_notice::busy(app) {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
        state.covers.load_or_make(&hash, Path::new(&path));
        made += 1;
    }
    made
}
