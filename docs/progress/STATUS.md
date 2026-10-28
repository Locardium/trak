# Estado actual — Beta 1 bugfix

> Leer esto primero en cada chat. Actualizar al terminar cada tarea.
> Detalle de cada ítem (causa raíz, archivos, fix): `PLAN-BETA1.md`. Historia: `LOG.md`.
> Lista original del usuario: `notes.txt` (raíz, gitignoreado).

**Última actualización:** 2026-10-27 (ajustes de audio/UI confirmados; release beta.2 en curso)
**Fase actual:** Fase 5 — release y docs. La app ahora se llama **Trak** (antes Sway, ver 5.3).
Fases 1-4 y 2 cerradas y confirmadas por el usuario.
**Git:** sólo `master`. Repo renombrado a `https://github.com/Locardium/trak` (`origin` ya
apunta ahí). Todo commiteado; el push lo hace el usuario (al cierre del 2026-10-14 iba a
pushear). La rama `stable` no existe hasta el primer `pnpm release`.
**Próximo paso (en este orden, actualizado 2026-10-14):**
1. Probar (usuario):
   - Android: `pnpm dev:android` (app nueva `com.locardium.trak.player.dev`; desinstalar la dev
     vieja para ver el ícono nuevo; hay que emparejar de nuevo).
   - Mover una carpeta de datos con música (portable o instalada) → los temas siguen sonando
     (`db::relocate_tracks`, 5.4).
   - Borrar un tema → aparece en `<carpeta de Trak>	rash\` (5.7).
   - Instalador ya probado por el usuario 2026-10-14 (instala en `E:\Programs\Trak`,
     desinstalar con/sin "Delete the application data" OK). El ícono "S" viejo que veía era la
     caché de íconos de Windows (`ie4uinit.exe -show`); el exe tiene el logo.
2. `pnpm release 1.0.0-beta.2` (antes `release:win`; bumpea, commitea, tag, build win+android+servers,
   `dist/v1.0.0-beta.2/`, `gh release create`, mueve `stable`). Necesita `JAVA_HOME` para el APK.
3. Server con Docker siguiendo `crates/server/README.md#install` (compila desde `stable`).
   Emparejar de nuevo todos los devices (cambió el contexto del hash del token, 5.3).
4. Mudanza manual del usuario: copiar `Music\Sway` → carpeta de Trak (`db.sqlite*`,
   `library\`, …); `relocate_tracks` arregla las rutas. Desinstalar Sway (PC y celu).
5. Pregunta abierta: ¿logo también dentro de la app (sidebar, pantalla vacía)? Si no, Fase 6.
6. Fase 6 (rediseño de UI) se planea aparte. 1.7 parte B (reanudación Media3) sigue en reserva.

- 🟡 Seguridad (2026-10-28, sin commit): path traversal al recibir archivos arreglado en
  `crates/core` (`transfer.rs`, `import.rs`, `engine.rs`), con tests; falta commitear y que entre en
  el próximo release (sin bump de protocolo). Detalle en LOG. Revisión de `db.rs`, `app/src-tauri/src`
  y front pendiente.

Antes de cada commit: `gen/android/app/build.gradle.kts` sale `w/mixed` por el CLI de Tauri →
`git checkout` de ese archivo.

**Todo lo de las fases 1-4 está commiteado en `master`** (ver LOG).
Ojo con los finales de línea: varios archivos del repo son CRLF en el índice (`git ls-files
--eol`); si un editor/script los deja en LF, el diff sale entero. Antes de commitear:
`git ls-files --eol | awk '$1=="i/crlf" && $2!="w/crlf"'` debe salir vacío.

**Protocolo de sync versionado desde 2.8**: hoy `wire::PROTOCOL` = 3, `MIN_PROTOCOL` = 3 (el
rename a Trak, 5.3, cambió mDNS y el hash del token). Builds de Sway salen "Update needed".
Regla de bump en `CLAUDE.md`.

**Builds en el S24:** release `com.locardium.sway` instalada 2026-09-25 14:24:46 (tiene 1.7A + 1.8
+ filtro de logs + 3.2 título/artista). Dev `com.locardium.sway.dev` = lo que haya corrido el
usuario con `pnpm dev:android`. adb: `192.168.0.6:5555` (LAN) o `100.109.147.99:5555` (Tailscale).
Build release para probar: `cd app && pnpm tauri android build --apk --target aarch64` con
`JAVA_HOME="C:\Program Files\Android\Android Studio\jbr"`, y `adb -s <dev> install -r <apk>`.

**Confirmado por el usuario en uso real (14 días con el APK de 1.0+1.1+1.5 en su S24, sin
crashes ni freezes):** 1.1 y 1.5 andan bien — "la UI no se freezeo más", "la app ya no se pierde
el estado, se abre exactamente como se dejó". Encontró dos bugs nuevos: 1.6 (abajo) y confirmó
que 3.2 (notificación sin nombre a veces) sigue activo.

**Estado del repo al arrancar el plan (no son cambios del plan):** había cambios sin commitear
previos en `.gitignore`, `gen/android/app/build.gradle.kts`, `lib.rs`, `tauri.conf.json` y el
rename de `MainActivity.kt` de `com/sway/app` a `com/locardium/sway`. No mezclarlos con commits
del plan sin preguntar.

Leyenda: ⬜ pendiente · 🟡 en curso · ✅ hecho y verificado · 🧪 hecho, falta probar en device

## Fase 1 — Estabilidad Android
- ✅ 1.0 Loop de reimport del watcher (notify 8 reporta OPEN de la carpeta → reimporta y rehashea
  toda la librería con el lock de DB tomado ~16 s, en loop). Hallazgo en device, fix verificado
  en device (I/O de ~3 GB/4 s a ~14 MB). Ver plan.
- ✅ 1.1 Freeze de la UI al subir el gain (LoudnessEnhancer a HandlerThread + knob sin re-render
  de tabla). Confirmado por el usuario en 14 días de uso real.
- ✅ 1.9 Emparejar: el código no aparece del otro lado y cerrar el panel congela la UI. 3 causas
  confirmadas con DevTools remoto (evento solo escuchado en Sync, colisión en `pending` por uid,
  Modal reusado que deja un backdrop invisible) — arreglado y confirmado por el usuario 2026-09-25.
- ✅ 1.8 Primer vínculo sincroniza todo sin preguntar + cambiar la selección en medio del sync no
  lo frenaba (ver plan). Hecho 2026-09-25, sin probar en device. Incluye filtro de logs de
  decodificadores en Android (ruido GEOB/RVAD).
- ✅ 1.2 (confirmado por el usuario 2026-10-13) Lag de la UI durante el sync (tick 25 ms, sync-progress sin throttle, recarga de librería, sweep R128)
- ✅ 1.3 (confirmado por el usuario 2026-10-13) Sync se corta con la pantalla apagada — FGS dataSync + wakelock + WifiLock hecho 2026-09-25; antes: exención de optimización de batería
  🧪 instalada en el S24 el 2026-09-25 (falta que el usuario la active desde Settings y confirme
  que sigue apareciendo tras un reinicio), falta el foreground service dataSync + wakelock +
  WifiLock (ver plan)
- ✅ 1.4 (confirmado por el usuario 2026-10-13) Transferencia rápida → lenta (hash dentro del lock de DB, library_changed por archivo, timeout 180 s)
- ✅ 1.5 Volver a la app = estado perdido (restore desde plugin/checkpoint + cola y playlist en DB
  + configChanges). Confirmado por el usuario en 14 días de uso real; servicio en foreground con
  pausa sigue afuera (ver plan), no hizo falta hasta ahora.
- ✅ 1.6 (confirmado por el usuario 2026-10-13) Botones de transporte mueren en background. Causa **confirmada** (exit-info): el proceso
  muere con la música pausada y al revivir solo queda el tema del checkpoint. El fallback nativo
  instalado el 2026-09-25 no arregla eso (casi nunca corre); el fix real es 1.7.
- ✅ 1.7 (parte A; B en reserva) Cola de reproducción nativa + reanudación tras muerte del proceso. **Parte A hecha; el
  usuario confirmó que la notificación "funciona perfecta" (2026-09-25)**. Falta probar la
  restauración tras matar el proceso. Detalle parte A: cola en `NativeAudioRuntime` con cues cacheados, persistida
  (archivo en `filesDir` + prefs), next/prev/pase automático nativos, botones → cola sin pasar
  por JS, restore al abrir la app, JS en Android solo empuja la cola y escucha eventos.
  **Parte B pendiente:** reanudación Media3 (`onPlaybackResumption` + `MediaButtonReceiver`) para
  botones con el proceso muerto SIN abrir la app.

## Fase 2 — Sync / servidor (2.1-2.5 hechas 2026-09-29, 2.6-2.8 2026-10-13; commiteada 2026-10-13, detalle en LOG)
- ✅ 2.1 Desconexión del server no se muestra (confirmado por el usuario 2026-09-29) — connect fallido marca gris al toque; heartbeat
  25 s / timeout 60 s. Probar: apagar el server (proceso y cable/red) → gris en segundos / ≤ ~65 s.
- ✅ 2.2 Aparece conectado antes de tiempo (confirmado por el usuario 2026-09-29) — estado "Checking…" hasta el primer probe; mDNS ya no
  pinta verde. Probar: abrir la app con PC/server prendidos y apagados.
- ✅ 2.3 (confirmado por el usuario 2026-10-09: token, badge, refresh, re-emparejar misma dirección) Pairing viejo no se olvida — badge rojo "Pairing broken" + "Forget"; re-emparejar un
  server en la misma dirección borra la fila vieja. Probar: borrar la DB del server, re-emparejar.
  Además (pedido del usuario): cambiar `pair_token` + reiniciar el server revoca los pairings
  hechos con el token viejo (la app lo olvida y hay que re-emparejar). Los pairings previos a este
  cambio no tienen token guardado → se revocan una vez al actualizar el server.
  Refresh ahora chequea todo (probe + Hello a cada emparejado) y un server "Needs token" tiene
  "Pair again" y "Remove".
- ✅ 2.4 (confirmado por el usuario 2026-10-09 en uso, sin mirar logcat) El celu recibe del server y de la PC a la vez — un sync por vez por device (`Msg::Busy` +
  reintento). Probar: cargar música en la PC con el celu cerrado, abrir el celu → log `[sync] ...
  waits` / `busy`, y que no reciba de los dos a la vez.

- ✅ 2.5 (reportado 2026-09-29) Playlists no seleccionadas se reenviaban vacías a devices nuevos, y
  quedaban en el celu tras desvincular al único que las tenía. Fix: `held` en el manifest +
  poda local de huérfanas (ver LOG). El usuario confirmó 2026-10-13 que marcar A/B y desvincular
  anda bien.
- ✅ 2.7 (reportado y hecho 2026-10-13, confirmado por el usuario en device 2026-10-13) Re-vincular
  aplicaba borrados hechos mientras estaban desvinculados. Causa: `changes_for_peer` manda todos
  los tombstones y `merge::apply` los aplicaba todos, sin mirar `devices.paired_at`; además un
  tombstone le ganaba siempre a tracks/playlists (no había forma de "mantener"). Fix: tombstone
  con `deleted_at < paired_at` del peer que lo manda, para algo que existe acá → tabla local
  `held_deletes` + diálogo "X deleted music you have here" (Delete here too / Keep and send
  back). Tombstones ahora LWW contra `updated_at` (tracks/playlists) → "Keep" sube el reloj y el
  ítem vuelve al otro. Server no pregunta (aplica como antes). Detalle en LOG. Probar: repro del
  usuario (PC borra A/B + temas con el celu desvinculado, re-vincular → diálogo en el celu, nada
  borrado; "Keep" → vuelven a la PC; repetir con "Delete" → papelera).
- ✅ 2.8 (hecho 2026-10-13, confirmado por el usuario entre devices 2026-10-13: badge "Update needed" y log del server) Versión de protocolo en core.
  `wire::PROTOCOL = 2`, `MIN_PROTOCOL = 2`, frame `Version` dentro de `Session::connect/accept`,
  error tipado `VersionMismatch`, badge "Update needed" + mensaje con quién, log del server, test
  `protocol_fingerprint`, regla en `CLAUDE.md`. Detalle en LOG. Diseño acordado (referencia):
  `PROTOCOL` + `MIN_PROTOCOL` en `sway-core`; intercambio `{protocolo, versión app}` como PRIMER
  frame tras el handshake Noise (antes de cualquier `Msg`, porque con protocolos distintos puede
  no parsear ni el `Hello`); igual → sync normal; distinto pero dentro de rango → sync con las
  features del más viejo (gating `peer_protocol >= N`); fuera de rango → no sincroniza, badge
  "Update needed" diciendo QUIÉN está atrás ("Sway Server is older — update sway-server") + log
  en el server. Cambios aditivos (campo `serde(default)`, mensaje nuevo) suben sólo `PROTOCOL`;
  rompientes suben `MIN_PROTOCOL`. **Para no olvidarse**: test con "huella" de todos los `Msg`
  serializados que falla si cambia el protocolo sin bump ("protocol changed: bump PROTOCOL (and
  MIN_PROTOCOL if it breaks), then update the fingerprint") + regla en `CLAUDE.md`. Reemplaza
  el `PROTO = "1"` de mDNS (`discovery.rs`, `SYNC_PROTO` en `api.ts`). Lo de la Fase 2 sale como
  protocolo 2 / mínimo 2: la release instalada (1.0.0-1, sin chequeo) se actualiza a mano una vez.
- ✅ 2.9 (reportado 2026-10-13, confirmado por el usuario 2026-10-13) En modo Selected, un tema agregado a una playlist
  quedaba gris hasta recargar: al agregar/sacar de una playlist `App.tsx` no recargaba la
  librería (de ahí sale `inScope`). Ahora sí (`onDropTracks`, modal "Add to playlist",
  `doRemoveFromPlaylist`). Probar: celu en Selected, playlist nueva, cargar tema, meterlo → color
  normal al toque.
- ✅ 2.10 (pedido 2026-10-13, confirmado por el usuario 2026-10-13) Re-vincular sin ningún device vinculado vuelve a
  preguntar "What should this device sync?" (antes sólo la primera vez en la vida del device; un
  device que ya había elegido Everything quedaba en Everything sin preguntar). Si ya estaba en
  Selected conserva sus marcas. La pregunta ahora es un diálogo en primer plano
  (`ScopePromptDialog`, a nivel App como el de Keep), ya no una fila dentro de Sync. Respuesta a
  la duda del usuario: al desvincular el último device NO se pasa a Everything (sin vínculos el
  scope no hace nada); se pregunta de nuevo al próximo vínculo.
- ✅ 2.12 (cerrado 2026-10-13: el usuario lo atribuye al modo dev; reabrir si pasa en release) En el celu Dev, tras importar un WAV de 114 MB con el
  selector del sistema mientras PC y server sincronizaban, la pantalla quedó negra ~4 s y la
  página se recargó (19:39:58 → 19:40:03). NO fue crash del proceso (mismo PID 2985 antes y
  después; Rust siguió sincronizando). Es una recarga del WebView: "[vite] connecting…" + "Couldn't
  find callback id" (respuestas de Rust a la página vieja). Logcat no muestra muerte del renderer,
  ni lmkd, ni cambio de archivos en `app/`, ni re-optimización de deps de Vite. Sospecha: algo
  del modo dev (servidor Vite / HMR). Siguiente paso: mirar la terminal de `pnpm dev:android` a
  esa hora (¿"page reload"/"server connection lost"?) y ver si pasa con un build release.
- ✅ 2.11 (cerrado 2026-10-13: no se repitió; era la playlist abierta en el panel izquierdo, no el tilde de Sync) "Keep me deselecciona la playlist que estaba
  seleccionada". En core NO pasa: test `keep_on_a_selective_phone_keeps_the_playlist_marked`
  (celu Selected, A marcada, PC borra A desvinculado, re-vincular, Keep → A sigue marcada) pasa.
  Falta aclarar con el usuario: ¿"seleccionada" = marcada en Sync o abierta en el sidebar? ¿en
  qué device? Posible relación con 2.10 (antes del cambio, re-vincular no tocaba el scope).
- ✅ 2.6 (hallado 2026-09-29, hecho 2026-10-13, confirmado en device 2026-10-13: log "not needed (another device is sending it)", una sola copia) Server recibía el mismo archivo de dos
  devices a la vez: doble subida y colisión del `.part`. Fix: `Msg::BlobPushAck { wanted }`
  antes de los bytes (no si ya lo tiene o si otra sesión lo está recibiendo, `claim_push`) +
  partial de push propio `<hash>.push.part`. Entra en el protocolo 2 (sin release todavía).
  Probar: celu y PC subiendo el mismo tema al server a la vez → log "not needed (another device
  is sending it)" y una sola copia.

## Fase 3 — Audio
- ✅ 3.1 Normalize satura — −14 LUFS + pico medido + techo −1 dBFS al boostear, hecho 2026-09-25
- ✅ 3.2 Notificación Android: título/artista + carátula (thumb cacheado), hecho 2026-09-25
- ✅ 3.3 Stop en la notificación + stop nativo único, hecho 2026-09-25 (botón de la UI sigue chico)
- 🧪 3.4 (reportado y hecho 2026-10-14, sin commit) Gapless deja silencio entre temas (PC y
  celu). Causa: el analizador topaba en 12 s el recorte de la cola, y el piso de −60 dB dejaba
  segundos de fade inaudible (ej. "PASO SOLITA": ~25 s bajo −60 al final). Fix (`loudness.rs`):
  cola sin tope y con piso −50 dBFS (`TAIL_FLOOR`); inicio sigue en −60 con tope 12 s.
  `db::migrate` re-mide toda la biblioteca una vez (`ANALYSIS_REV` = 3, setting `analysis_rev`).
  Usuario confirmó en PC dev 2026-10-14 que con el tope sacado anda; pidió cortar antes (−50),
  eso falta probarlo. Celu: relanzar `pnpm dev:android`. Detalle en LOG.

- ✅ 3.5 (2026-10-27, confirmado por el usuario) Normalize más fuerte: `TARGET_LUFS` −14 → −10
  (`crates/core/src/db.rs`). Con normalize ON el trim (knob Gain) queda deshabilitado y no afecta el
  audio; el valor guardado se conserva. `playback_gain_db` ignora `gain_db` si `normalize`.
- ✅ 3.4 gapless con piso −50 dB: sin quejas desde 2026-10-14.
- ✅ 2026-10-27 UI: la última playlist del sidebar queda encima del player (`.tree flex: 1 0 auto`,
  spacer `.sidebar::after`, `--player-clear` medido por `PlayerBar.tsx`, 0 sin player). El volumen
  guardado se manda al backend al abrir (`useUiSetting` `onLoad`). Confirmados por el usuario.

## Fase 4 — Desktop visual / flujo
- ✅ 4.1 Música aparece en la otra playlist — estado `{id, tracks}` + request counter (2026-09-25)
- ✅ 4.2 Carátulas: caché en disco + pre-generado en background + cola (4 en vuelo, LIFO,
  cancelación) + observer con root `.table-wrap` y margen 600 px. Scroll confirmado por el
  usuario. Borrado de carátulas de temas borrados (poda en el pase de background).
- ✅ 4.3 Reveal in Explorer: `raw_arg` en Windows, `FileManager1.ShowItems` en Linux. Confirmado.
- ✅ 4.4 Drag & drop: drop del OS en la posición + indicador; drag hacia afuera con el crate
  `drag` (comando `drag_tracks_out`). Confirmado (adentro, afuera, al Explorer).
- ✅ 4.6 (extra, reportado 2026-09-25) Menú de click derecho del sidebar se cortaba abajo en vez
  de abrirse hacia arriba. Sidebar ahora usa `ContextMenu` (flip arriba/izquierda + portal a body).
- ✅ 4.5 Título/artista siempre visibles: min 180 px, título en 2 líneas, apila bajo 760 px.
  El usuario lo deja así; el detalle fino va a la Fase 6.

Fase 4 confirmada por el usuario en desktop 2026-09-29 ("anda todo lo que debería andar").

## Fase 5 — Release y docs
- 🧪 5.1 (hecho 2026-10-13, sin commit, sin correr) `scripts/release.js` (antes `release-win.js`) reescrito: sin
  la conversión de versión de WiX (NSIS acepta `1.0.0-beta.2`; Tauri usa sólo los números en
  `VIProductVersion`), `cargo update --workspace` tras el bump, build win + android + servers,
  staging en `dist/<tag>/` con nombres fijos (`Trak-setup.exe`, `Trak-portable.exe`,
  `Trak.apk`, `trak-server-windows.exe`, `trak-server-linux`; sólo archivos generados en esta
  corrida) y `gh release create` desde ahí. `bump-version.js` ahora también bumpea
  `crates/server/Cargo.toml`. `dist/` ya estaba ignorado (`**/dist/`). Imagen Docker NO se
  publica desde el script (no hay Docker acá).
- 🧪 5.2 (hecho 2026-10-13, sin commit) README raíz: secciones "Install" (instalador,
  portable, APK, datos junto al programa) y "Running the Server" (quickstart Linux + Docker),
  build:win ya no dice `.msi`, `pnpm release`. `crates/server/README.md`: "Quick start on
  Linux", nota de `TRAK_SERVER_TOKEN` al primer arranque, protocolo 3, sección Docker.
  `crates/server/config.example.toml` (generado del template de `config.rs`).
  README corregido contra el código (2026-10-13): −14 LUFS (`db::TARGET_LUFS`) con techo
  −1 dBFS, papelera 30 días en devices (`trash::RETENTION_DAYS`) / 90 en el server, server sin
  axum/tokio, canal Noise XX, Rust "latest stable".
- 🧪 5.3 (hecho 2026-10-13, sin commit, falta probar en device) Rename Sway → Trak en todo el
  código (65 archivos, ver LOG). Crates `trak-core`/`trak-server`/`trak` (lib `trak_lib`),
  mDNS `_trak._tcp`, env `TRAK_DEVICE_NAME`/`TRAK_SERVER_TOKEN`, Android
  `com.locardium.trak.player` (dev `.dev`, app NUEVA: desinstalar Sway y re-sincronizar),
  `PROTOCOL`/`MIN_PROTOCOL` = 3. También el contexto del hash del token (`"trak pairing token v1"`,
  `crates/core/src/pairing.rs`): los pairings guardados en el server quedan inválidos → el
  device muestra "Pairing broken" (2.3) y hay que emparejar de nuevo una vez. NO se renombró: `docs/`, `tools/`, la
  carpeta del repo ni el repo de GitHub (`Locardium/sway`, lo renombra el usuario después). Tests core,
  server, app y `tsc` pasan. Falta: `pnpm dev:android` (paquete Kotlin movido a
  `com/locardium/trak/player`, se borró el `generated/` viejo; el CLI lo regenera).
- 🧪 5.4 (pedido 2026-10-13, hecho sin commit) Portable + instalable, TODO directo en la
  carpeta del exe (sin subcarpeta `data`). Hecho: `app/src-tauri/src/data_dir.rs` (carpeta =
  la del exe; dev = `target/app-dev`; AppImage = carpeta del .AppImage; chequeo de escritura
  con MessageBox si no se puede; sufijo del identifier por carpeta → lock de instancia única
  por carpeta), `lib.rs` sin `DATA_DIR` ni `adopt_legacy_webview_profile`, comando
  `data_folder` + fila "Data folder" en Settings (desktop). Instalador NSIS perMachine
  (`tauri.conf.json` `bundle.windows.nsis`, `build:win --bundles nsis`, sin MSI) con
  `app/src-tauri/windows/hooks.nsh`: icacls Users Modify en `$INSTDIR`, `Trak.exe` y
  `uninstall.exe` sin herencia (Users RX). El desinstalador de Tauri 2.11.4 sólo borra el exe,
  el uninstaller y `RMDir $INSTDIR` sin /r → los datos quedan (verificado en el template).
  Probado: `pnpm build:win` arma `Trak_1.0.0-1_x64-setup.exe` (el `.nsi` generado incluye
  `hooks.nsh`, `MAINBINARYNAME` = `trak`); portable copiado a una carpeta y abierto con otro
  cwd → `db.sqlite`, `library/`, `covers/`, `webview/` al lado del exe, nada en
  `%LOCALAPPDATA%`; en carpeta con escritura denegada → MessageBox y no crea nada.
  Usuario 2026-10-13: la casilla "Delete the application data" del desinstalador no borraba
  nada (Tauri sólo limpia `%APPDATA%`/`%LOCALAPPDATA%\<bundle id>`). Agregado
  `NSIS_HOOK_POSTUNINSTALL` en `hooks.nsh`: con la casilla y sin `/UPDATE` borra por nombre
  db/wal/shm, perf.log, `library`, `covers`, `trash`, `incoming`, `webview` y `RMDir` sin /r
  de `$INSTDIR`. Confirmado por el usuario 2026-10-13. Commit `6934012`.
  Bug hallado 2026-10-13 (pregunta del usuario por USB/portable): `tracks.path` es absoluto y
  nada lo corregía si la carpeta de datos se mueve (portable movido, otra letra de USB, la
  mudanza manual de `Music\Sway`, `music_dir` del server en otro disco) → todos los temas
  "faltantes". Fix: `db::relocate_tracks` (core) reescribe `path = music_dir/rel_path` al
  arrancar, en la app (después de `backfill_rel_paths`) y en el server. Test
  `relocate_points_tracks_at_the_new_folder`. Commit `6934012`. Falta probar moviendo una
  carpeta real con música.
  Sin migración: el usuario mueve `Music\Sway` a mano (contenido de esa carpeta → carpeta de
  Trak). Ver PLAN 5.4.
  Además, en 5.6: con `TRAK_SERVER_TOKEN` seteado y sin config, el server arranca con
  defaults en vez de escribir el config y salir (pedido del usuario).
- 🧪 5.7 (pedido 2026-10-13, commit `e6c91c3`) Todo borrado va a la papelera de Trak (`trash\`, 30
  días): `db::delete_tracks` usa `trash::move_to_trash` en vez del crate `trash` (Papelera de
  Windows; en Android era `remove_file` definitivo; en USB Windows no tiene Papelera →
  definitivo). Crate `trash` sacado de `crates/core/Cargo.toml`. Bug de paso:
  `move_to_trash` conservaba el mtime original y `purge_old` mira el mtime → un tema viejo se
  purgaba en el próximo arranque; ahora se pone el mtime en "ahora" al moverlo (test
  `an_old_file_gets_its_full_retention`). Texto del diálogo de borrar (App.tsx) actualizado.
  Falta: botón "Restore" en la UI (Fase 6); hoy se recupera a mano desde `trash\`.
- 🧪 5.6 (hecho 2026-10-13, sin commit, SIN PROBAR: no hay Docker en esta PC)
  `crates/server/Dockerfile` (rust:1.96-alpine → alpine:3.22, usuario 10001, volumen
  `/srv/trak`, workdir ahí), `crates/server/docker-compose.yml` (token desde `.env`),
  `.dockerignore` en la raíz. Server: si no hay config y está `TRAK_SERVER_TOKEN`, escribe el
  config con defaults y `pair_token = ""` y arranca (`Config::load_or_create_with`, test
  `with_the_token_in_the_environment_the_first_run_starts`). Publicación (2026-10-13, sin
  commit): sin registry ni GitHub Actions (el usuario no los quiere; tiene Docker en el
  server). `docker-compose.yml` compila la imagen EN el server desde
  `https://github.com/Locardium/trak.git#${TRAK_VERSION:-stable}`. La rama `stable` la mueve
  `release.js` (paso 8, `git push --force origin <sha del tag>:refs/heads/stable`, sólo
  tras publicar, sin rollback si falla) → por defecto el server usa el último release;
  `TRAK_VERSION=vX` en `.env` lo fija. Actualizar = `docker compose up -d --build`. `.env`
  sólo necesita `TRAK_SERVER_TOKEN`. `trak-data` hay que crearlo con `chown 10001:10001`
  (si lo crea Docker es de root y el server no puede escribir). Commit `7f0224d`. Sin probar.
- 🧪 5.5 (pedido y hecho 2026-10-13, sin commit) Licencia BSL 1.1: `LICENSE` en la raíz (texto
  oficial de SPDX `BUSL-1.1`, Terms idénticos), Licensor Locardium, Additional Use Grant = uso
  personal, pruebas y self-hosting gratis; prohibido uso comercial. Change Date 4 años por
  versión → Apache 2.0. README "License" + `license = "BUSL-1.1"` en Cargo/package.json.
  `crates/native-audio` sigue MIT/Apache. Falta: que el usuario revise el texto; GitHub detecta
  la licencia al hacer push.

- ✅ Ícono (2026-10-14): logo `app/src-tauri/icons/trak-logo.png` → `pnpm tauri
  icon` + `python scripts/android-icon.py` (Android adaptativo con margen y fondo oscuro; ver
  LOG). Exe e instalador verificados con el logo (`build.rs` rerun por `icons/` +
  `installerIcon`). Falta verlo en el celu.

## Fase 6 — Rediseño de UI
- ⬜ Se planea aparte cuando terminen las fases 1-5.

## Quedó a medias / notas abiertas
- 🧪 Dev de Windows aislado del release (2026-09-29, sin commit): `<Music>\Sway Dev`,
  identifier `.dev`, nombre `<PC> Dev` (XML de iTunes compartido, a propósito). Ver LOG. Falta correrlo con el release
  abierto al mismo tiempo.
- 1.6 y la exención de batería (1.3 parcial) ya están instalados en el S24 (2026-09-25, adb sobre
  LAN 192.168.0.6:5555 — Tailscale no respondió esa vez, no hace falta investigarlo). Falta la
  prueba de uso real: activar la exención en Settings, y ver si next/prev por Bluetooth/
  notificación sigue funcionando después de un buen rato en background con la app fuera de
  pantalla.
- Sigue instalada en el celu la app vieja `com.sway.app` (0.1.0) — pendiente que el usuario la
  borre, no urgente.
- Versión: `1.0.0-beta.2` desde 2026-10-13 (app, tauri.conf, server; `bump-version.js` +
  `cargo update --workspace`). Próximo release: `pnpm release 1.0.0-beta.2`. Semver: la
  1.0.0 final no salió, así que las betas siguen `1.0.0-beta.N` (no `1.0.1-beta`).
