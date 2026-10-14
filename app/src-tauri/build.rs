fn main() {
    // The Windows exe's icon is compiled into it here (resource.lib), but
    // tauri-build only asks to rerun when tauri.conf.json or the capabilities
    // change: a new logo in icons/ left the exe, and the installed app, with
    // the old one while the window icon was already new.
    println!("cargo:rerun-if-changed=icons");
    tauri_build::build()
}
