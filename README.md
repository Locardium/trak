# Trak 🎧

> **A music library manager and player built by a DJ, for DJs, with love ❤️**

Trak is currently in **Beta**. Since this is a passion project, expect some bugs! I welcome all suggestions, bug reports, and code contributions to make it better.

---

## The Story Behind Trak (Why I built it)

As a DJ, organizing a local music collection (FLACs, MP3s, WAVs) can be a real headache. For years, I relied on **iTunes** to curate my library. It had a simple, powerful way to order tracks manually, and by exporting an iTunes XML file, I could easily import my structured playlists and folders directly into legacy DJ software like **Rekordbox** and **Serato**.

However, iTunes eventually presented three critical dealbreakers:
1. **It became obsolete**: Apple replaced iTunes with Apple Music, which completely removed the iTunes XML export feature, breaking my entire DJ library prep workflow.
2. **Unsupported formats**: It lacked native support for high-quality formats like **FLAC**, forcing annoying file conversions.
3. **Impossibly complex sync**: Trying to keep my music library synchronized across my two PCs and my Android phone was incredibly frustrating, slow, or outright impossible.

I built **Trak** to solve these exact problems. It is a modern, lightweight desktop and mobile application designed to replace the iTunes/Explorer flow, allowing DJs to easily manage, pre-listen, sync, and project their playlist hierarchies onto legacy DJ platforms.

---

## Key Features

* **Legacy Export First**: Trak natively creates standard iTunes-compatible XML files, ensuring your manual playlist orders and folder hierarchies carry over perfectly to Rekordbox and Serato.
* **All Formats Welcome**: Native support for **FLAC**, MP3, WAV, and more. No conversions needed.
* **Safe-by-Design File Integrity**: Your files are sacred. Anything Trak deletes — by you or by a sync — goes to its own trash first and stays there for 30 days (90 on the server) before it's really removed. You'll never accidentally lose a track from your hard drive again.
* **Real-time P2P Synchronization**: Your PCs and your phone find each other on the local network and sync tracks and playlists directly, device to device (P2P), end-to-end encrypted.
* **Your Own Backup & Remote Server** (optional): Run a headless server that keeps a backup of every track and playlist from all your devices. It also bridges them: when two devices aren't on the same network, each one syncs with the server and the changes reach the other through it, even if the other one is offline at the time.
* **Advanced Player & Volume Normalization**: Pre-listen to your sets with seamless crossfades, gapless playback, and smart volume normalization targeting **-10 LUFS** without pushing peaks past -1 dBFS (no more sudden volume jumps between tracks!).

---

## Install

Downloads are on the [latest release](https://github.com/Locardium/trak/releases/latest).

* **Windows, installer** (`Trak-setup.exe`): installs to `C:\Program Files\Trak` by default, or
  any folder you pick. It asks for admin once, to make that folder writable; Trak itself runs
  without admin.
* **Windows, portable** (`Trak-portable.exe`): no installer. Put it in any folder you can write
  to and open it.
* **Android** (`Trak.apk`).

On Windows, Trak keeps **everything next to the program**: the database, the `library` folder
with your music, covers and its trash. Back up or move that folder and you've moved your whole
library. Uninstalling removes the program and leaves that data where it is, unless you tick
"Delete the application data".

## Running the Server

The server is optional: a box that keeps a copy of everything your devices send it, and lets
them sync when they aren't on the same network. It runs on Linux, two ways:

* **Docker**: download one compose file, set a token, `docker compose up -d --build`. Docker
  builds it from this repo at the latest release.
* **systemd**: one static binary from the release, no Docker or Rust needed.

Install, update and remove, step by step for both: [`crates/server/README.md`](crates/server/README.md#install).

---

## Future Roadmap 🚀

Here is what is planned for future releases:
- **UI/UX Redesign** (Coming soon): Improving layout density, shortcuts, and overall looks.
- **Native BPM Scanning**: Analyze tempo directly inside the app.
- **Comprehensive Docs**: Written guides on workflow setups.
- **macOS & iOS Support**: Planned for future phases once the project gains traction (currently limited by testing hardware!).

---

## Technical Overview (For Developers)

Trak is built as a Rust and pnpm monorepo using **Tauri v2** and **React 18**.

### Project Structure
```
trak/
├── app/                  # Tauri frontend application (Vite + React + TS)
│   ├── src/              # React UI (TrackTable, PlayerBar, Sync, etc.)
│   └── src-tauri/        # Tauri application backend (desktop playback, watchers, platform APIs)
├── crates/
│   ├── core/             # Central engine (SQLite db, sync logic, encryption, ID3, wire protocol)
│   ├── native-audio/     # Custom fork of tauri-plugin-native-audio for Android/iOS playback
│   └── server/           # Headless archive and sync server (no UI, runs anywhere, Docker too)
└── scripts/              # Release automation scripts
```

- **`crates/core`**: Decoupled engine handling SQLite database merges (`rusqlite`), `blake3` file hashing, metadata parsing, and the versioned sync protocol over an encrypted TCP channel (**Noise XX**, ChaCha20-Poly1305; `wire.rs`).
- **`crates/server`**: Headless daemon (a single static binary, or a Docker image) designed to run on a Linux box or VPS as a central backup and sync node.
- **`app/src-tauri`**: Controls OS-specific file watchers, loudness analysis, and iTunes XML generation.
- **`app/`**: A Vite single-page application communicating with Tauri's Rust API. On Android, it routes playback commands natively to Media3/ExoPlayer.

---

## Build & Setup Instructions

### Prerequisites
Make sure you have installed:
- **Rust** (latest stable)
- **Node.js** 18+ and **pnpm**
- **Android SDK** (if testing/building for Android)

### Install Node Dependencies
```bash
pnpm install
```

### Running in Development

- **Run Desktop App**:
  ```bash
  pnpm run dev
  ```
- **Run Android App**:
  ```bash
  pnpm run dev:android
  ```
- **Run Headless Server**:
  ```bash
  pnpm run server:dev
  ```

### Compiling Release Bundles

- **Build Windows Desktop Installer (NSIS `-setup.exe`; `target/release/trak.exe` is the portable build)**:
  ```bash
  pnpm run build:win
  ```
- **Build Android App (`.apk`)**:
  ```bash
  pnpm run build:android
  ```
- **Build Linux Server** (uses `cargo-zigbuild`):
  ```bash
  pnpm run server:build-linux
  ```
- **Build Windows Server**:
  ```bash
  pnpm run server:build-win
  ```
- **Build All Targets**:
  ```bash
  pnpm run build:all
  ```
- **Publish a Release** (bumps the version, builds everything into `dist/<tag>/`, creates the
  GitHub release):
  ```bash
  pnpm release 1.0.0-beta.2
  ```

---

## License

Licensed under the [Business Source License 1.1](LICENSE).

- **Free:** personal use, testing and evaluation, and self-hosting (running Trak and its server
  on your own machines, for yourself or your own organization).
- **Not allowed:** any commercial use — distributing it commercially, selling it or its
  binaries, or offering it (or a service built on it) to third parties as a paid product.
- Each version becomes available under the Apache License 2.0 four years after its release.

`crates/native-audio` is a vendored fork and keeps its original MIT OR Apache-2.0 licenses.
