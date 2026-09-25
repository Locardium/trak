//! Sway's fork of `tauri-plugin-native-audio` — see `FORK.md`.
//!
//! Almost everything this crate does lives in Kotlin (Android) and Swift
//! (iOS). Commands the webview calls are declared in `build.rs` and reach the
//! platform code by name, without passing through here. The exception is the
//! sync notice below, which the app's Rust drives directly: a sync runs on a
//! Rust thread, often with no webview looking.

use tauri::{
    plugin::{Builder, TauriPlugin},
    AppHandle, Runtime,
};

#[cfg(target_os = "android")]
use tauri::Manager;

#[cfg(target_os = "android")]
const PLUGIN_IDENTIFIER: &str = "app.tauri.nativeaudio";

#[cfg(target_os = "ios")]
tauri::ios_plugin_binding!(init_plugin_native_audio);

/// Handle to the Kotlin plugin, kept so Rust can call it without a webview.
#[cfg(target_os = "android")]
struct NativeAudio<R: Runtime>(tauri::plugin::PluginHandle<R>);

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("native-audio")
        .setup(|_app, _api| {
            #[cfg(target_os = "android")]
            {
                let handle = _api.register_android_plugin(PLUGIN_IDENTIFIER, "NativeAudioPlugin")?;
                _app.manage(NativeAudio(handle));
            }
            #[cfg(target_os = "ios")]
            {
                let _ = _api.register_ios_plugin(init_plugin_native_audio)?;
            }
            Ok(())
        })
        .build()
}

/// What the work notification says: a sync, or the loudness sweep.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncNotice {
    pub title: String,
    pub text: String,
    /// Position in the run.
    pub done: u32,
    pub total: u32,
    /// "upload", "download" or "analyze".
    pub icon: String,
}

/// Shows (or updates) the sync notification. On Android this runs a
/// foreground service of type `dataSync` that also holds a partial wakelock
/// and a Wi-Fi lock, so the system doesn't cut a sync off with the screen off
/// (Fase 1.3). No-op on other platforms.
///
/// Blocks for a round trip to the UI thread: call it from a worker thread,
/// throttled, never from the main thread.
pub fn show_sync_notice<R: Runtime>(app: &AppHandle<R>, notice: &SyncNotice) {
    #[cfg(target_os = "android")]
    if let Some(p) = app.try_state::<NativeAudio<R>>() {
        if let Err(e) = p.0.run_mobile_plugin::<serde_json::Value>("showSyncNotice", notice) {
            log::warn!("[sync] could not show the sync notification: {e}");
        }
    }
    #[cfg(not(target_os = "android"))]
    let _ = (app, notice);
}

/// Removes the sync notification and releases the locks. No-op off Android.
pub fn hide_sync_notice<R: Runtime>(app: &AppHandle<R>) {
    #[cfg(target_os = "android")]
    if let Some(p) = app.try_state::<NativeAudio<R>>() {
        if let Err(e) = p.0.run_mobile_plugin::<serde_json::Value>("hideSyncNotice", ()) {
            log::warn!("[sync] could not hide the sync notification: {e}");
        }
    }
    #[cfg(not(target_os = "android"))]
    let _ = app;
}
