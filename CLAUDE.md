# Sway — instrucciones para Claude

## Continuidad entre chats (OBLIGATORIO)

El usuario trabaja en varios chats. Cada chat nuevo tiene que saber dónde quedó el anterior.

**Al empezar cualquier chat en este repo:**
1. Leer `docs/progress/STATUS.md` — dónde estamos, qué sigue, qué quedó a medias.
2. Si la tarea es del plan beta, leer la sección correspondiente de `docs/progress/PLAN-BETA1.md`
   (causa raíz + archivos + fix ya investigados; no re-investigar desde cero).
3. Verificar contra el código antes de confiar en un `archivo:línea` del plan: las líneas se mueven.

**Mientras se trabaja y al terminar cada tarea (no sólo al final del chat):**
1. Actualizar `docs/progress/STATUS.md`: estado del ítem (⬜ pendiente / 🟡 en curso / ✅ hecho /
   🧪 hecho sin probar en device), "Próximo paso", y cualquier cosa que haya quedado a medias.
2. Agregar una entrada al principio de `docs/progress/LOG.md` con: fecha, ítem, qué se cambió
   (archivos), commit si hubo, cómo se verificó, y hallazgos/gotchas que no están en el código.
3. Si una causa raíz resultó distinta a la del plan, corregir el plan (no dejar info falsa).

Escribir para un Claude que no vio esta conversación: concreto, con rutas, sin "como vimos antes".

## Reglas del proyecto

- Node: **siempre `pnpm`**, nunca `npm`.
- UI visible al usuario: **en inglés**. Documentación interna (`docs/progress`): en español.
- Nunca sostener el lock de SQLite mientras se hashea o se hace I/O de archivos.
- Android: todo `@Command` del plugin `crates/native-audio` corre en el UI thread; nada bloqueante ahí.
- Android: `gen/android` no lo mira el watcher de `tauri android dev` — cambios en Kotlin/gradle
  requieren cortar y relanzar. Usar `pnpm dev:android` (ya trae `--no-watch`).
- No usar AppData para datos de Sway.
- Android: dev y release son apps separadas. `pnpm dev:android` instala `com.locardium.sway.dev`
  ("Sway Dev", firmada con la clave debug); la release es `com.locardium.sway` (clave Locardium).
  El sufijo sale de `bundle.android.debugApplicationIdSuffix` en `tauri.conf.json`: el CLI de Tauri
  reescribe `applicationIdSuffix` en `build.gradle.kts` en cada corrida, no editarlo a mano. Nunca
  instalar un build debug sobre la release (firma distinta → habria que desinstalar → se borra la
  biblioteca del celu).
- Commits/push sólo cuando el usuario lo pida.

## Mapa rápido

- `app/src` — React+TS (App.tsx concentra estado; componentes en `components/`).
- `app/src-tauri/src` — Rust de la app (comandos en `lib.rs`, sync en `pairing.rs`/`autosync.rs`/
  `watch.rs`/`discovery.rs`, audio desktop en `player.rs`, R128 en `loudness.rs`).
- `app/src-tauri/gen/android` — Android nativo, commiteado a mano (MainActivity, manifest).
- `crates/core` — motor de sync sin Tauri (`engine.rs`, `wire.rs`, `transfer.rs`, `db.rs`).
- `crates/native-audio` — fork del plugin Media3 (Kotlin en `android/src/main/java/...`).
- `crates/server` — server headless (doc completa en `crates/server/README.md`).
- Tests: `cargo test -p sway-core`, `cargo test -p sway-server`; front: `cd app && pnpm tsc --noEmit`.
