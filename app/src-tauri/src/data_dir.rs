//! Where Trak keeps what it owns on desktop (Phase 5.4): the db, `library/`,
//! `covers/`, `trash/`, `incoming/` and the webview profile, all loose in the
//! folder the executable runs from.
//!
//! One rule for both builds. The portable `Trak.exe` keeps its library
//! wherever it is opened; the installed one keeps it in the install folder,
//! which the installer made writable (`windows/hooks.nsh`) so the app never
//! has to run elevated. Moving or backing up the folder moves everything.
//!
//! Android doesn't come through here: its data lives in the app's private
//! storage (see `setup` in lib.rs).

use std::path::{Path, PathBuf};

/// The folder this run keeps its data in.
///
/// Dev builds (`tauri dev`, debug assertions on) run from `target/debug`,
/// in the middle of the build output, so they get a fixed folder in the repo
/// instead, next to the dev server's (`target/server-dev`).
pub fn resolve() -> PathBuf {
    if cfg!(debug_assertions) {
        return Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("app/src-tauri sits two levels below the repo root")
            .join("target")
            .join("app-dev");
    }
    // An AppImage runs from a read-only mount; the folder that matters is
    // the one holding the .AppImage file.
    #[cfg(target_os = "linux")]
    if let Some(dir) = std::env::var_os("APPIMAGE").and_then(|p| Path::new(&p).parent().map(Path::to_path_buf)) {
        return dir;
    }
    let exe = std::env::current_exe().expect("path of the running executable");
    exe.parent().expect("the executable is inside a folder").to_path_buf()
}

/// Proves the folder can be written to before anything opens the db there.
/// Without this a portable copy dropped into a protected folder (Program
/// Files, a read-only share) would die on `db open` with nothing on screen.
pub fn check_writable(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let probe = dir.join(".trak-write-test");
    std::fs::write(&probe, b"")?;
    std::fs::remove_file(&probe)
}

/// Suffix for the identifier, which the single-instance lock is derived
/// from: one lock per data folder. Two copies of Trak in different folders
/// are two libraries and may run side by side; a second launch from the same
/// folder still focuses the running window.
pub fn instance_suffix(dir: &Path) -> String {
    let path = dir.to_string_lossy();
    // Windows paths are case-insensitive: `C:\Trak` and `c:\trak` are the
    // same folder and must get the same lock.
    let path = if cfg!(windows) { path.to_lowercase() } else { path.into_owned() };
    format!("{:08x}", crate::export_xml::fnv1a(&path) as u32)
}

/// Tells the user why Trak can't start and exits. Runs before any window
/// exists, so on Windows it is a plain message box.
pub fn fail_not_writable(dir: &Path, err: &std::io::Error) -> ! {
    let msg = format!(
        "Trak can't write to its folder:\n{}\n\n{err}\n\n\
         Trak keeps its library next to the program. Move Trak to a folder \
         you can write to, or install it with the installer.",
        dir.display()
    );
    eprintln!("[data] {msg}");
    #[cfg(target_os = "windows")]
    {
        use windows::core::HSTRING;
        use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
        // SAFETY: plain blocking call with valid, owned strings and no
        // owner window.
        unsafe {
            MessageBoxW(None, &HSTRING::from(msg), &HSTRING::from("Trak"), MB_OK | MB_ICONERROR);
        }
    }
    std::process::exit(1);
}
