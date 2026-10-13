// Command names are snake_case here and in `permissions/`, but the Kotlin
// methods they reach are lowerCamelCase: tauri runs the name through
// `heck::AsLowerCamelCase` before handing it to the Android plugin
// (`webview/mod.rs`, the `#[cfg(mobile)]` fallback), and `PluginHandle` looks
// the method up by its exact Kotlin name. So `set_next_source` -> `setNextSource`.
const COMMANDS: &[&str] = &[
    "initialize",
    "register_listener",
    "remove_listener",
    "set_source",
    "play",
    "pause",
    "seek_to",
    "set_rate",
    "get_state",
    "get_progress_checkpoint",
    "clear_progress_checkpoint",
    "dispose",
    // Added by the Trak fork, see FORK.md.
    "set_volume",
    "set_source_gain",
    "set_next_source",
    "skip_to_next",
    "set_crossfade",
    "list_output_devices",
    "set_output_device",
    // Battery-optimization exemption (Fase 1.3). Not audio-specific, but this
    // is the only Android plugin Trak has wired up, and it exists for the
    // same reason as everything else here: without it Android can freeze or
    // kill the process this plugin's foreground service runs in, which is
    // what silently drops transport buttons pressed after that point — see
    // `NativeAudioRuntime.transport` on the Kotlin side.
    "is_ignoring_battery_optimizations",
    "request_ignore_battery_optimizations",
    // Native play queue (Fase 1.7): the player owns next/previous/end of
    // track, so a button pressed with the app off screen or after the
    // process was killed doesn't depend on JS.
    "set_queue",
    // Stop from the app or the notification (Fase 3.3): the player owns it
    // like it owns the queue.
    "stop",
    "set_queue_mode",
    "queue_next",
    "queue_prev",
    "restore_queue",
];

fn main() {
    tauri_plugin::Builder::new(COMMANDS)
        .android_path("android")
        .ios_path("ios")
        .build();
}
