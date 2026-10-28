# Log de trabajo

> Entradas nuevas ARRIBA. Una por tarea terminada (o por sesión si quedó a medias).
> Formato: fecha · ítem · qué se cambió (archivos) · commit · cómo se verificó · hallazgos.

---

## 2026-10-27 · `release:win` pasa a `release`

- Pedido del usuario: un solo comando de release para todo. `scripts/release-win.js` → `scripts/release.js`,
  `pnpm release <versión>` (sin alias viejo). Ya construía instalador + portable de Windows, APK, server Windows y
  Linux (musl) y publica en GitHub; Docker no se construye acá: el server compila desde la rama `stable`, que el
  script mueve al tag al final. Referencias actualizadas (README, STATUS, PLAN-BETA1). Sin cambios de lógica.

---

## 2026-10-27 · Volumen no se restauraba al reabrir la app

- Bug del usuario: bajar el volumen, cerrar y abrir → el slider queda en lo guardado pero el audio suena al 100%.
  Causa: el efecto de carga inicial de `App.tsx` mandaba `setVolumeBackend(volume)` con el `1` inicial del estado,
  antes de que `useUiSetting` leyera el valor guardado. Fix: opción `onLoad` en `uiSettings.ts` (se llama con el valor
  restaurado) y `App.tsx` la usa para mandarlo al backend; se sacó el envío del efecto inicial.
  `tsc` pasa. Sin probar en la app. Sin commit.

---

## 2026-10-27 · Normalize volume más fuerte (TARGET_LUFS -14 → -10)

- Pedido del usuario: normalize bajaba demasiado muchos temas (la biblioteca está entre -4 y -8 LUFS,
  así que con -14 todo se cortaba 6-10 dB). `crates/core/src/db.rs`: `TARGET_LUFS = -10.0`; tests de
  `playback_gain_db` ajustados a los nuevos números; README dice -10 LUFS.
- Los boosts siguen limitados por `PEAK_CEILING_DB` (-1 dBFS), así que subir el objetivo no vuelve a
  saturar (3.1). Si todavía suena bajo: subir a -9/-8 o hacerlo una opción en Settings.
- No sube `PROTOCOL` (es local, no viaja). `cargo test -p trak-core` pasa. Sin commit. Sin probar de oído.
- Mismo día, pedido del usuario: con normalize ON el trim (knob Gain) se deshabilita y NO afecta el audio;
  el valor guardado (`tracks.gain_db`) se conserva y vuelve a aplicar al apagar normalize.
  `db::playback_gain_db` ignora `gain_db` si `normalize` (tema sin medir = 0 dB). `Knob.tsx` prop `disabled`,
  `PlayerBar.tsx` prop `normalize`, `App.tsx` pasa `prefs.normalize`, `styles.css` `.knob.disabled`.
  Tests de `playback_gain_db` actualizados. `cargo test -p trak-core` y `tsc` pasan. Sin probar en la app.

- Mismo día: con muchas playlists en el sidebar las últimas filas quedaban tapadas por el player flotante.
  Causa probable: `--player-clear` (96px / 148px en `styles.css`) era menor que el alto real del player
  (título de 2 líneas + inset). Fix: `PlayerBar.tsx` mide el footer con ResizeObserver y setea
  `--player-clear` en `:root` (alto + bottom + 12px); al desmontar lo borra y vuelve el valor del CSS.
  `tsc` pasa. Sin probar en la app (no se vio el scroll real: si sigue tapando, medir el alto del footer en DevTools).
  El usuario reportó que seguía sin verse la última playlist: el `padding-bottom` del `.sidebar` (flex column con
  scroll) no se cuenta en el alto scrolleable. Ahora es un spacer `.sidebar::after { height: var(--player-clear) }`
  (`styles.css`). `.table-wrap` (bloque) sigue con padding. Sin probar.
  Seguía sin scrollear: causa real = `.tree` era `flex: 1` (basis 0) → caja del alto sobrante y su contenido
  desbordaba sin agrandar el scroll del sidebar (quedaba bajo el player y no había scroll). Fix: `.tree { flex: 1 0 auto }`
  para que mida su contenido y spacer/padding sumen al scroll. Sin probar.
  Confirmado por el usuario que ya se ve. Pedido: sin player no debe haber scroll de más → `--player-clear` default
  `0px` (se quitó el 96/148px del CSS; `PlayerBar.tsx` lo setea mientras está montado); toast y right-panel usan
  `max(var(--player-clear), var(--player-inset))` para no pegarse al borde. Sin probar.

---

## 2026-10-14 · 3.4 Gapless deja silencio entre temas

- Reporte: silencio entre temas con gapless, en PC y Android.
- Diagnóstico con la DB real (`E:\Programs\Trak\db.sqlite`, copiada a temp) + niveles de los WAV
  (script Python con `wave`, pico por ventana): "2 - PASO SOLITA…" tenía
  `duration_ms - audio_end_ms` = 12000 exacto = `MAX_TRIM_MS`. El archivo cae bajo −60 dBFS
  ~25 s antes del final (−48 a 29 s, −60 a 25 s, −87 al final) → con el tope se tocaban ~13 s
  de silencio. Los otros dos temas no tenían cola larga. Ambas plataformas usan los mismos
  números (`cue_for` desktop, `cue_for_track` → Media3 `ClippingConfiguration`), por eso falla
  en las dos. El mecanismo de encadenado (rodio `Sink` append / playlist de ExoPlayer) estaba
  bien: no se tocó.
- Fix: `app/src-tauri/src/loudness.rs` — `MAX_TRIM_MS` sólo para el inicio (una intro tranquila
  es música); la cola se recorta entera. `crates/core/src/db.rs` — en `migrate`, una sola vez
  (setting local `analysis_rev`), se borran las mediciones de filas con
  `duration_ms - audio_end_ms >= 11000` para que el sweep las re-mida. Sin marca se re-medirían
  en cada arranque (una cola de 25 s sigue cumpliendo la condición).
- Tests: `loudness::tests::a_long_silent_tail_is_trimmed_whole`,
  `db::tests::tails_cut_at_the_old_cap_are_remeasured_once`. `cargo test -p trak-core` y
  `cargo test --lib loudness` (en `app/src-tauri`) OK. Sin probar escuchando.
- Usuario probó en PC dev (`target/app-dev`, ya tenía `analysis_rev=2`): "sí funcionó", pero
  pidió cortar antes, a −50. Antes de eso se verificó que el encadenado de rodio no mete hueco:
  test descartable con `Sink::new_idle()` + los WAV reales, PASO SOLITA → COQUETA, el formato
  cambia 44.1k→48k justo en el borde sin samples de silencio. El silencio que quedaba era el
  fade del propio tema (−40 a −60 dB los últimos ~9 s antes del corte).
- Segundo cambio: `TAIL_FLOOR` = −50 dBFS sólo para el final (`scan_edges` lleva `last_tail`
  aparte); el inicio sigue en −60 para no comer fade-ins. Si un tema nunca pasa de −50, el final
  cae al de −60 (no se corta entero). Migración reescrita: `ANALYSIS_REV` = 3 en `db.rs`; si
  `analysis_rev` guardado < 3 → `clear_analysis` (re-mide TODA la biblioteca una vez; en el celu
  tarda minutos, con notificación). Para el próximo cambio del analizador: subir `ANALYSIS_REV`.
- Tests nuevos: `a_faded_tail_under_the_tail_floor_is_trimmed`,
  `a_quiet_track_is_not_cut_to_nothing`, `db::tests::measurements_from_an_older_analyzer_are_redone_once`
  (reemplaza al de la migración por tope). `cargo test -p trak-core` y `cargo test --lib loudness` OK.

## 2026-10-14 · Ícono de la app: logo de Trak

- El usuario dejó `app/src-tauri/icons/trak-logo.png` (2000×2000, cuadrado oscuro redondeado
  con la "t", esquinas transparentes). `pnpm tauri icon src-tauri/icons/trak-logo.png` (desde
  `app/`) regeneró todos los íconos (`icons/*`, `.ico`, `.icns`, iOS) y los `mipmap-*` de
  `gen/android`, más `mipmap-anydpi-v26/` + `values/ic_launcher_background.xml` (nuevos: antes
  Android no tenía ícono adaptativo).
- Problema: en Android Tauri pone el logo de borde a borde en el foreground con fondo `#fff`
  (`tauri icon` no tiene opción de fondo ni margen; sólo `--ios-color`). Con la máscara del
  launcher (72 de 108dp) la "t" quedaba enorme y se veían esquinas blancas.
- Fix: `scripts/android-icon.py` (Pillow + numpy). Toma el centro opaco del logo
  (`OPAQUE` 200..1800), lo escala para que la esquina más lejana de la "t" (`GLYPH_BOX` medido:
  595,351–1382,1651) quede al 75 % del radio visible, y extiende el fondo repitiendo los bordes
  (`np.pad mode=edge`) para que el degradé siga sin costura. Fondo `#1d1d1d`. Verificado con
  previews de máscara círculo / squircle / cuadrado. **Después de cada `pnpm tauri icon` hay que
  correr `python scripts/android-icon.py`**; con otro logo, medir de nuevo `OPAQUE` y
  `GLYPH_BOX`. Sin probar en el celu.
- Usuario: el exe instalado seguía con el ícono viejo (sólo la ventana tenía el nuevo). Causa:
  el ícono del exe se compila en `resource.lib` desde el `build.rs`, y `tauri-build` sólo
  pide rerun por `tauri.conf.json`/capabilities, no por `icons/`. Fix:
  `app/src-tauri/build.rs` → `cargo:rerun-if-changed=icons`. Además el instalador NSIS tenía
  el ícono genérico de NSIS: `bundle.windows.nsis.installerIcon = "icons/icon.ico"`.
  Verificado extrayendo el ícono (`System.Drawing.Icon.ExtractAssociatedIcon`) de
  `target/release/trak.exe` y de `Trak_1.0.0-beta.2_x64-setup.exe`: los dos con el logo. Si
  Windows sigue mostrando el viejo en un acceso directo es su caché de íconos.

## 2026-10-13 · README del server reorganizado (instalar/actualizar/quitar)

- `crates/server/README.md`: nueva sección "Install" con "With Docker" y "With systemd (no
  Docker)", cada una con Install / Logs / Update / Remove (y cómo borrar también los datos).
  Se fusionaron "Quick start on Linux", "Docker" y "systemd"; "Running" → "Configuration";
  "Building" → "Building from source" al final (con el `docker build` desde el repo). En
  systemd el update para el servicio antes del `curl` (binario en uso no se pisa).
- `README.md` raíz: "Running the Server" resumido, link a `crates/server/README.md#install`.

## 2026-10-13 · 5.6 compose usa el último release (rama `stable`) + README roadmap

- Pedido: no tener que poner `TRAK_VERSION`. Git no tiene "último release", así que
  `scripts/release-win.js` paso 8 mueve la rama `stable` al commit del tag recién publicado
  (no fatal: si falla, imprime el comando a mano). `docker-compose.yml`: contexto
  `#${TRAK_VERSION:-stable}`, imagen `trak-server:${TRAK_VERSION:-stable}`. La rama no existe
  hasta el primer release con este script.
- Hallado al escribir los pasos: el bind mount `./trak-data` que crea Docker queda de root y el
  contenedor corre como uid 10001 → no puede escribir. Documentado `mkdir trak-data && sudo
  chown 10001:10001 trak-data` (compose, ambos READMEs).
- README raíz: roadmap "UI/UX Polish" → "UI/UX Redesign".

## 2026-10-13 · 5.6 Docker: el server compila la imagen desde GitHub + repo renombrado

- El usuario renombró el repo a `https://github.com/Locardium/trak`; `git remote set-url origin`
  hecho. El remoto `master` estaba en `63272b3` (todo lo de Fase 2 en adelante sin push).
- Se probó la idea de publicar en GHCR con un workflow de GitHub Actions; el usuario la
  descartó (no quiere Actions ni registry; tiene Docker en el server). Borrado.
- `crates/server/docker-compose.yml`: `build.context` =
  `https://github.com/Locardium/trak.git#${TRAK_VERSION}` + `dockerfile:
  crates/server/Dockerfile`, `image: trak-server:${TRAK_VERSION}`. En el server alcanza con
  el compose y un `.env` (`TRAK_VERSION`, `TRAK_SERVER_TOKEN`). READMEs actualizados.
  Requiere BuildKit (default en Compose v2) por el `RUN --mount=type=cache` del Dockerfile.
- Arreglado: en `crates/server/README.md` el `\` de continuación del `curl` de Linux se había
  perdido. Sin probar Docker (no hay en esta PC).

## 2026-10-13 · 5.7 borrar manda a la papelera de Trak + retención real de 30 días

- Antes: borrar desde la app → Papelera de Windows (`trash::delete`, crate `trash`), en
  Android `remove_file` (definitivo); borrados por sync → `trash/` de Trak. En un USB Windows
  no hay Papelera: el "reciclado" era definitivo.
- Ahora `crates/core/src/db.rs` `delete_tracks` → `crate::trash::move_to_trash` en todas las
  plataformas; dependencia `trash` eliminada (`crates/core/Cargo.toml`, `Cargo.lock`).
- Bug: `trash::purge_old` borra por mtime y `move_to_trash` (rename/copy) conservaba el mtime
  del archivo → la música vieja se purgaba al siguiente arranque. Fix: `set_modified(now)` tras
  mover (best effort). Test nuevo.
- `App.tsx`: diálogo "Their files go to Trak's trash for 30 days; files outside the library
  are left on disk." El README ya decía eso (ahora es cierto también para borrados manuales).
- Tests: core 141, server, app 57, `tsc` OK.

## 2026-10-13 · Rutas de temas al mover la carpeta de datos (`db::relocate_tracks`)

- Hallazgo: la tabla `tracks` guarda `path` absoluto (UNIQUE) y `rel_path` (nombre dentro de
  la biblioteca, plana). Ningún código reconstruía `path` desde `rel_path` al arrancar: sólo
  `transfer`/`scope` lo hacen al recibir un archivo. Mover la carpeta (portable en USB con otra
  letra, copiar `Music\Sway` a la carpeta de Trak como se planeó, cambiar `music_dir` del
  server) dejaba todos los temas apuntando al lugar viejo.
- Fix: `crates/core/src/db.rs` `relocate_tracks(conn, music_dir)` (una transacción, sólo filas
  con `rel_path`, sólo las que difieren). Llamado en `app/src-tauri/src/lib.rs` setup después
  de `backfill_rel_paths` (antes del watcher) y en `crates/server/src/main.rs` después de
  `set_device_name`. Test en core. Tests: core 140, server, app 57 OK.
- Respuesta al usuario sobre instalar el NSIS en un USB: funciona (en FAT32/exFAT `icacls`
  falla sin cortar la instalación y no hace falta: no hay permisos), pero el acceso directo y
  la entrada de "Programs" quedan atados a la letra; para USB conviene el portable.

## 2026-10-13 · 5.4 desinstalador: "Delete the application data" ahora borra los datos de Trak

- Reporte del usuario: marcando la casilla, la biblioteca quedaba. Causa: el template NSIS de
  Tauri 2.11.4 sólo hace `RmDir /r` de `%APPDATA%` y `%LOCALAPPDATA%\<bundle id>`.
- Fix: `NSIS_HOOK_POSTUNINSTALL` en `app/src-tauri/windows/hooks.nsh`, misma condición que
  el template (`$DeleteAppDataCheckboxState = 1` y `$UpdateMode <> 1`), borra sólo lo que crea
  Trak por nombre y después `RMDir "$INSTDIR"` sin `/r` (por si se instaló en una carpeta con
  otras cosas). Las variables del template se pueden usar en el hook porque la macro se
  expande dentro de `Section Uninstall`. `pnpm build:win` OK →
  `Trak_1.0.0-beta.2_x64-setup.exe`. Sin probar (UAC).

## 2026-10-13 · 5.4 portable/instalador, 5.1 release, 5.2 READMEs, 5.6 Docker (sin commit)

- 5.4: `app/src-tauri/src/data_dir.rs` nuevo (`resolve`, `check_writable`, `instance_suffix`,
  `fail_not_writable` con `MessageBoxW`; feature `Win32_UI_WindowsAndMessaging` en `windows`).
  `lib.rs`: fuera `DATA_DIR`, `adopt_legacy_webview_profile`, `remove_dir_all_best_effort`,
  `copy_dir_all`; `context(data_dir)` agrega `.<fnv32 de la carpeta>` al identifier (antes
  `.dev` en debug) → lock de instancia única por carpeta; comando `data_folder`.
  `export_xml::fnv1a` pasó a `pub(crate)`. Settings: fila "Data folder" (desktop) en vez de
  `<Music>/Trak` + botón "Change…" deshabilitado. `tauri.conf.json`: `bundle.windows.nsis`
  perMachine + `installerHooks: ./windows/hooks.nsh`; `build:win` → `--bundles nsis`.
- Verificación 5.4: ver STATUS (portable con cwd distinto, carpeta sin escritura, `.nsi`
  generado). Template NSIS de Tauri 2.11.4 revisado: el desinstalador borra sólo el exe y el
  uninstaller y hace `RMDir "$INSTDIR"` sin `/r`; la casilla "Delete app data" sólo borra
  `%APPDATA%`/`%LOCALAPPDATA%\<bundle id>` (nada nuestro). La página de reinstalación corre el
  uninstaller viejo antes de instalar (mismo efecto: los datos quedan).
- Gotcha: dev desktop ahora guarda en `target/app-dev` (lo borra `cargo clean`, igual que
  `target/server-dev`).
- 5.6 server: `LEGACY_CONFIG` vuelve a `"sway-server.toml"` (el rename mecánico lo había
  dejado en `trak-server.toml`, un nombre que nunca existió; es el nombre viejo real). El
  default del server siempre fue `config.toml` (corrige lo que se le dijo al usuario antes:
  no hay que renombrar nada a `trak-server.toml`).
- Tests: core 139, server 11+4+13+7, app 57, `tsc` OK.

## 2026-10-13 · 5.3 rename Sway → Trak (sin commit)

- Reemplazo mecánico en archivos trackeados (sin `docs/`, `tools/`, `graphify-out/`,
  `Cargo.lock`): `com.locardium.sway` → `com.locardium.trak.player`, `com.sway.app` →
  `com.trak.app` (buildSrc), y `sway/Sway/SWAY` → `trak/Trak/TRAK`. 65 archivos.
- `git mv` de `MainActivity.kt` a `gen/android/app/src/main/java/com/locardium/trak/player/` y
  de `buildSrc/.../com/sway/app/kotlin` a `com/trak/app/kotlin`. Borrado el `generated/` viejo
  (ignorado por git, lo regenera el CLI de Tauri).
- `blake3::derive_key("trak pairing token v1")` en `crates/core/src/pairing.rs`: primero se
  dejó en "sway", el usuario pidió cambiarlo. Efecto: los `token_hash` guardados en el server
  ya no coinciden → el device ve "Pairing broken" (flujo de 2.3) y se re-empareja una vez. No es
  silencioso, y con `MIN_PROTOCOL` = 3 hay que actualizar todo igual.
- `PRODUCT.md`: sacado "evoking sway (movement, water)", ya no aplica.
- Borrado `flatten_legacy_subdir` de `app/src-tauri/src/lib.rs` (migración Android de
  `<managed>/Sway`; con app id nuevo no existe ese layout).
- `wire::PROTOCOL` y `MIN_PROTOCOL` = 3 (mDNS `_trak._tcp`: un Sway viejo ya no se ve; por
  conexión directa sale "Update needed"). Huella: mismos mensajes, sólo cambia el número.
- Verificado: `cargo test -p trak-core` (139 ok), `-p trak-server`, `-p trak` (57 ok),
  `pnpm tsc --noEmit`. Sin probar Android ni el release.
- Gotchas para el usuario: el server lee ahora `trak-server.toml` y `TRAK_SERVER_TOKEN`
  (renombrar su config a mano); el keystore de release sigue siendo `sway-release.keystore`,
  alias `sway` en `key.properties` (no trackeado, no se tocó; firma igual).

## 2026-10-13 · Fase 5 ampliada (5.3 rename, 5.4 portable/instalable) + 5.5 licencia BSL 1.1

- Pedidos nuevos en Fase 5: rename Sway → Trak (5.3), build portable + instalable con datos
  junto al exe y fuera de `<Music>` (5.4), licencia BSL 1.1 (5.5). Decisiones en STATUS/PLAN.
- 5.5 hecho, sin commit: `LICENSE` nuevo (texto bajado de
  `raw.githubusercontent.com/spdx/license-list-data/main/text/BUSL-1.1.txt`; verificado con diff
  que la sección Terms quedó idéntica), `README.md` sección License, `license = "BUSL-1.1"` en
  `crates/core`, `crates/server`, `app/src-tauri` Cargo.toml y en `package.json`/`app/package.json`.
  `cargo metadata` OK.
- Gotcha legal: BSL no deja que el Additional Use Grant restrinja los derechos base, y la base
  ya permite copiar y redistribuir (uso no productivo). "Prohibido vender binarios" no es 100%
  exigible con BSL: quien los compre sólo podría usarlos dentro del grant. Si importa, la
  alternativa es PolyForm Noncommercial. Change Date y Change License (compatible con GPL) son
  obligatorios en BSL: se eligió 4 años → Apache 2.0.

## 2026-10-13 · Fase 2 cerrada en device

- Usuario: 2.8 "listo perfecto" (badge "Update needed" + línea en el log del server). 2.11 no
  se repitió; "seleccionada" era la playlist abierta en el panel izquierdo (no el tilde de Sync):
  cerrado. 2.12 lo atribuye al modo dev (Vite): cerrado, reabrir si pasa en un build release.
- Commiteado en `d76327f fix: scope prompt on re-link, grey tracks, server log counter` (2.9,
  2.10, contador del log del server, test de 2.11, docs). Sin push.

## 2026-10-13 · 2.6 confirmado + recarga del WebView en el celu (2.12)

- 2.6 confirmado en device: celu y PC Dev subiendo el mismo WAV (113.909.850 bytes) al server
  dev → log del server `not needed (another device is sending it)`, un solo `done`.
- 2.12 (nuevo, sin causa): en el celu, tras elegir el WAV con el photopicker, pantalla negra ~4 s
  y la página se recargó. Investigado con `adb logcat -d -v time` (S24 en `192.168.0.6:5555`):
  el proceso `com.locardium.sway.dev` (PID 2985) no murió; el último frame del WebView es
  19:39:58.8 y a las 19:40:03.1 aparece `[vite] connecting...` (carga nueva de la página),
  después muchos `[TAURI] Couldn't find callback id …`. No hay `chromium`/renderer crash, ni
  `lmkd`/kill, ni archivos de `app/` modificados, ni re-optimización de Vite
  (`node_modules/.vite/deps/_metadata.json` de agosto, mismo `browserHash`). No hay
  `location.reload` en el front. Pendiente: terminal de `pnpm dev:android` a esa hora, y probar
  con release.
- Hallazgo menor en el mismo log: la PC le empujó al celu el mismo WAV que el celu acababa de
  importar (19:39:57 `received … 113909850 bytes` en el celu): el celu todavía no tenía el hash
  calculado de su copia (otro uid), así que el plan no lo reconoció. `insert_received` lo
  deduplica (borra la copia redundante); es tráfico desperdiciado, no un error. Sin tocar.

## 2026-10-13 · Confirmaciones del usuario en device

- Confirmados: 2.7 (Keep/Delete al re-vincular), 2.9 (tema gris), 2.10 (pregunta de scope al
  re-vincular, cartel en primer plano). También dice que 1.2-1.7 ya estaban confirmados (STATUS
  los tenía en 🧪): marcados ✅.
- Pendiente: probar 2.8 y 2.6 (se le pasaron pasos), aclarar 2.11. 2.9/2.10 siguen sin commit.
- Log del server dev con protocolo 2 anda (`protocol 2 (accepts 2+)`, sync PC dev → server ok).
  Bug cosmético encontrado ahí: el server logueaba "receiving 2/3 … 4/3" porque
  `crates/server/src/host.rs progress` sumaba `+ 1` a un `index` que ya es 1-based (el que
  manda hace `index += 1` antes de usarlo). Corregido, sin commit.
- 2.8 A: en la ventana de la release (1.0.0-1) la Dev sale "Other version" con Pair
  deshabilitado: es el comportamiento VIEJO (ese texto ya no existe en el código nuevo); el
  badge nuevo "Update needed" se ve en la ventana Sway Dev.

## 2026-10-13 · Reportes nuevos tras la Fase 2: 2.9, 2.10, 2.11 (sin commit)

- 2.9 tema gris en modo Selected: `inScope` de cada tema viene de `list_tracks` (librería) y
  agregar/sacar de una playlist sólo recargaba playlists + temas de la playlist. `App.tsx`: los
  tres caminos (`onDropTracks`, modal de agregar, `doRemoveFromPlaylist`) ahora también
  `refreshLibrary()`.
- 2.10 prompt de scope al re-vincular: `app pairing.rs is_first_pairing` = "no hay ningún device
  en `devices`" (antes además exigía que este device nunca hubiera elegido scope). En
  `store_device`, si el modo previo era All marca todas las playlists propias (como antes); si ya
  era Selected deja sus marcas. UI: componente nuevo `components/ScopePromptDialog.tsx` en
  `App.tsx` (escucha `pairing-done`/`peers-changed`, "Choose playlists" abre Sync); se sacó la
  fila `scope-prompt` de `Sync.tsx` y su CSS. `lib.rs resolve_scope_prompt` emite
  `peers-changed` para que un Sync abierto relea el scope.
- 2.11 "Keep deselecciona la playlist": no reproduce en core (test nuevo
  `engine::tests::keep_on_a_selective_phone_keeps_the_playlist_marked` pasa). Pendiente de
  aclarar con el usuario.
- Gotcha de entorno: `pnpm dev:android` falló con "rustc-LLVM ERROR: out of memory" con 9 GB de
  RAM libre: el límite que se llenó es la memoria **confirmada** (commit charge ~30/32 GB;
  rust-analyzer ~4 GB y mysqld ~2,4 GB reservados casi sin uso). Fix: cerrar rust-analyzer/
  MySQL, o agrandar el archivo de paginación (lo hace el usuario).
- Verificado: `cargo test -p sway-core`, `cargo check` app, `pnpm tsc --noEmit`. Sin probar en
  device.

## 2026-10-13 · Commits de la Fase 2

- Dos commits en `master` (pedido del usuario: separar): `c0c8618 chore(desktop): isolate dev
  builds from the release` (sólo los hunks de "dev de Windows aislado" de `lib.rs`, separados con
  un patch + blob re-escrito a CRLF: `git apply --cached` dejó las líneas nuevas en LF), y después
  el commit con toda la Fase 2 (2.1-2.8 + docs). Sin push.
- `package.json`: entra el `server:dev` = `cd target/server-dev && cargo run -p sway-server --
  config.toml` (cambio de otro chat no registrado: los datos del server dev viven en
  `target/server-dev/`, no en la raíz). La devDependency `shx` que venía con ese cambio (sin uso
  en ningún script) se sacó por decisión del usuario; `pnpm-lock.yaml` restaurado a HEAD.
- `gen/android/app/build.gradle.kts` restaurado con `git checkout` (ruido del CLI de Tauri).

## 2026-10-13 · 2.6 Doble subida al server del mismo archivo

- Sin commit (va con toda la Fase 2).
- Causa (ya conocida, 2026-09-29): el server atiende varias sesiones; dos devices que suben el
  mismo hash a la vez lo mandaban dos veces y escribían el mismo `incoming/<hash>.part` → hash
  mismatch y reintento.
- Fix (`crates/core`): `Msg::BlobPushAck { wanted }` (`wire.rs`): tras cada `BlobPush` el
  receptor (`engine::serve_requests`) contesta antes de cualquier byte: `false` si ya tiene el
  hash presente (`transfer::path_for_hash`) o si otra sesión lo está recibiendo
  (`transfer::claim_push`: registro en memoria del proceso por partial, liberado en `Drop`). El
  que manda (`engine::sync`) lee el ack; si es `false` lo saltea (no cuenta como enviado ni
  fallido; el próximo sync lo ve ahí). Partial de push propio `incoming/<hash>.push.part`
  (`push_part_path`): antes compartía `<hash>.part` con los pulls y un push borraba una descarga
  a medias del mismo archivo. `receive_file` cambió el parámetro `resuming` por `push`.
- Protocolo: sigue en 2 (todavía sin release); se actualizó sólo la huella de
  `protocol_fingerprint` (el test la detectó, como tenía que hacer).
- Test nuevo `engine::tests::a_file_already_coming_in_is_not_pushed_twice`; actualizado
  `transfer::tests::a_stale_partial_does_not_poison_a_push` (ahora además verifica que el `.part`
  del pull sobrevive). Verificado: `cargo test -p sway-core -p sway-server`, `cargo check` app.
  Sin probar con devices reales.

## 2026-10-13 · 2.8 Versión de protocolo

- Sin commit (va con toda la Fase 2).
- `crates/core/src/wire.rs`: `PROTOCOL = 2`, `MIN_PROTOCOL = 2`, `compat(theirs, their_min) ->
  Compat {Ok, PeerOlder, WeAreOlder}`, `set_app_version` (OnceLock; la app lo setea en `.setup`
  con `package_info().version`, el server con `CARGO_PKG_VERSION`). Frame `Version {protocol,
  minProtocol, app}` que **ambos** lados mandan y después leen, dentro de `Session::finish`
  (o sea en `connect` y `accept`: ningún llamador puede saltearlo). Fuera de rango →
  `VersionMismatch {theirs: Option<u32>, their_app, peer_is_older}`. Un build sin versiones
  (protocolo 1) se detecta así: si conecta él, lo primero que manda es un `Hello` (no parsea como
  `Version`); si conectamos nosotros, no entiende nuestro frame y corta → `is_disconnect` justo
  después del handshake = "el otro es viejo" (`theirs: None`). Sesión ok guarda
  `peer_protocol`/`peer_app` para gatear features futuras.
- Test `wire::tests::protocol_fingerprint`: serializa una muestra de **cada** `Msg` con todos los
  campos (struct literals → un campo nuevo no compila sin agregarlo; `match` sin wildcard → una
  variante nueva tampoco) + el `Version`, blake3, compara con `FINGERPRINT = (PROTOCOL, hash)`.
  Mensaje: "protocol changed: bump PROTOCOL (and MIN_PROTOCOL if it breaks older peers), then
  update FINGERPRINT in wire.rs to (N, "...")". Más `compat_says_who_is_behind` y
  `a_peer_without_versions_is_reported_as_older`. Regla nueva en `CLAUDE.md`.
- Server (`serve.rs`): log claro por mismatch ("runs a newer Sway (...): update sway-server" /
  "runs an older Sway (...): update the app on that device"); `main.rs` loguea versión y
  protocolo al arrancar. README del server: sección "Versions".
- App: `discovery.rs` — `Peer.proto` reemplazado por `Peer.update: Option<String>` (mensaje
  listo). mDNS publica `proto` = PROTOCOL y `minproto` = MIN_PROTOCOL; `announced_update` lo
  compara (un peer viejo publica `proto=1` sin `minproto` → "older"). `Peers.outdated` (mapa por
  uid, de sesiones fallidas) pisa al anuncio; `is_invalid` ahora es "no se puede sincronizar
  hasta que alguien actúe" (roto o versión) → watcher/autosync lo dejan en paz; `clear_invalid`
  limpia los dos (`pairing_ok` lo llama al abrir sesión bien). `update_note(name, platform,
  peer_is_older, their_app)` arma el texto ("X runs an older Sway Server — update sway-server",
  "X runs a newer version of Sway — update this app", ...). `pairing.rs session_failed`:
  `open_session_with` y `connect_inner` marcan outdated + devuelven `PairingBroken(nota)`;
  `pair_with_server` devuelve la nota como error del diálogo. `autosync.rs` filtra
  `update.is_none()`. UI (`api.ts`, `Sync.tsx`): `SYNC_PROTO` borrado, badge rojo "Update
  needed" + la nota en la lista y en la vista del device; sin botones de sync.
- **Protocolo 2 todavía no salió**: 2.6 (que cambia mensajes) se mete en el mismo protocolo 2,
  sólo hay que actualizar la huella. Después del primer release con protocolo 2, cualquier
  cambio sube el número.
- Verificado: `cargo test -p sway-core -p sway-server`, `cargo test --lib` en app (57),
  `pnpm tsc --noEmit`. Sin probar entre devices (probar: server viejo vs app nueva → "Update
  needed" en la app y línea en el log del server).

## 2026-10-13 · 2.7 Re-vincular aplicaba borrados hechos mientras estaban desvinculados

- Sin commit (va con toda la Fase 2).
- Causa (verificada en código): `merge::changes_for_peer` manda **todos** los tombstones locales
  en cada sync y `merge::apply` aplicaba **todos** los que llegaban; `devices.paired_at` no se
  miraba. Al desvincular el celu conserva temas/playlists y la PC sus tombstones → primer sync del
  vínculo nuevo los aplica. Además un tombstone de track/playlist le ganaba siempre al ítem (sólo
  las memberships comparaban `added_at`), así que "mantener y reenviar" era imposible.
- Fix (`crates/core`):
  - `merge::apply(conn, changes, music_dir, confirm_from: Option<&str>)`. Por tombstone,
    `target()` mira qué toca acá: `Gone` (no existe → se guarda como siempre), `Newer` (el ítem
    tiene `updated_at`/`added_at` posterior → el tombstone pierde, NO se guarda), `Present` → si
    `confirm_from` y `deleted_at < paired_at` de ese peer → fila en tabla local nueva
    `held_deletes(entity, uid, deleted_at, from_device)` (no se guarda en `tombstones`, no viaja),
    `Applied.held += 1`; si no, se guarda y aplica como antes.
  - **Ojo nombre**: la tabla NO puede llamarse `pending_deletes`: `db::migrate` hace
    `DROP TABLE IF EXISTS pending_deletes` (política vieja de la 6.4) en cada arranque.
  - Tombstones LWW: `manifest::plan` (pull/push de archivos y conteo de playlists) y
    `changes_for_peer`/`apply` de playlists sólo respetan el tombstone si `deleted_at >=
    updated_at`; si el ítem es más nuevo, se borra el tombstone viejo (`apply` para playlists,
    `transfer::insert_received` para tracks).
  - `merge::pending_deletes(conn)` (resumen por device, poda filas que ya no aplican) y
    `merge::resolve_pending(conn, music_dir, device, keep)`: delete = guarda + aplica (papelera);
    keep = `updated_at`/`added_at` = now → en el próximo sync gana contra el tombstone del otro y
    vuelve (también vía server o cualquier tercero: no depende de quién pasa el tombstone).
  - `Host::confirms_deletions()` (default `false`; app `true`; server no tiene UI → aplica como
    antes) y `Host::deletions_held()` (app emite `pending-deletes`). Llamado en `engine::sync` y en
    `serve_requests` (MetaPush): los dos lados preguntan.
  - `core::pairing::forget_device` borra los `held_deletes` de ese device (desvinculado = no se
    aplica nada; si se vuelve a vincular, se vuelve a preguntar).
- App: comandos `pending_deletes` / `resolve_pending_deletes` (`lib.rs`; tras resolver:
  `library-changed`, `autosync::note_change`, y `kick_background` si borró). UI: componente nuevo
  `components/HeldDeletesDialog.tsx` montado en `App.tsx` junto a `PairingDialog` (aparece con
  cualquier pantalla abierta; cerrar con la X deja todo retenido y vuelve a preguntar en el
  próximo `pending-deletes` o al abrir la app). `api.ts`: `pendingDeletes`,
  `resolvePendingDeletes`.
- Semántica nueva a tener en cuenta: un tema/playlist editado en un device después de que otro
  lo borró (sin verse) revive. Coherente con "ante la duda, conservar". Comparación de relojes
  entre devices distintos (`deleted_at` del que borró vs `paired_at` local): un desfase de
  minutos sólo importa si se borra justo al vincular.
- Caso server: si el celu sigue vinculado al server y el server a la PC, los borrados de la PC
  llegan por el server y se aplican solos (el celu nunca dejó el grupo). Decisión explicada al
  usuario; no se tocó.
- Verificado: `cargo test -p sway-core` (tests nuevos
  `engine::tests::re_linking_holds_deletes_made_while_unlinked_and_keep_sends_them_back` = repro
  del usuario + keep, y `held_deletes_can_be_confirmed_and_linked_deletes_still_apply`),
  `cargo test -p sway-server`, `cargo test --lib` en app (57), `pnpm tsc --noEmit`. Sin probar
  en device.
- Gotcha: `cargo check` de la app con jobs por defecto se quedó sin memoria (rustc OOM /
  STATUS_STACK_BUFFER_OVERRUN); con `-j 2` anda.

## 2026-10-13 · Pruebas de Fase 2 + bug nuevo 2.7 + diseño de versiones (2.8)

- Confirmado por el usuario: 2.1, 2.2, 2.3 (token, badge, refresh, re-emparejar en la misma
  dirección), 2.4 (en uso, sin mirar logcat), 2.5 (marcar A/B y desvincular).
- Bug nuevo 2.7 (repro y lo esperado en STATUS): re-vincular aplica en el celu los borrados que la
  PC hizo mientras estaban desvinculados. Sin investigar todavía.
- Decidido 2.8: versión de protocolo con rango (`PROTOCOL`/`MIN_PROTOCOL`) + test de huella; diseño
  completo en STATUS. Aclaración del usuario: no busca romper compatibilidad, busca que un update
  nunca rompa en silencio.
- Nada de código en esta entrada; todo lo de Fase 2 sigue sin commit.

## 2026-09-29 · Playlists no seleccionadas se reenviaban a devices nuevos (bug reportado)

- Reporte: celu dev (scope selectivo, emparejado antes con la PC release) → desvinculó la PC
  release, vinculó PC dev + server dev → en la PC dev aparecieron **vacías** las playlists que el
  celu no tenía seleccionadas (eran de la PC release).
- Causa: por diseño (doc de `scope.rs`) las playlists/metadata "se replicaban completas": el celu
  guardaba el catálogo de todo lo que vio y `merge::changes_for_peer` se lo pasaba entero a
  cualquiera. Vacías porque los temas sólo viajan con archivo.
- Fix (`crates/core`): `PlaylistEntry.held` (manifest; `scope::mark_held`: modo All → todas; modo
  Selected → marcadas + descendientes + carpetas ancestro). `changes_for_peer` y los contadores del
  plan no crean del otro lado playlists/memberships que el emisor no tiene. Tabla local nueva
  `playlist_sources(playlist_uid, device_uid)` (no se replica): al final de cada sync saliente
  (`engine::sync`) `scope::note_sources` guarda qué tiene el peer y `scope::prune_orphans` borra
  **local, sin tombstone** las playlists no-held que ningún device vinculado tiene y sin ningún tema
  presente acá. `core::pairing::forget_device` borra las fuentes del device; `app pairing.rs
  forget_device` poda y emite `library-changed`. Primer vínculo (`store_device` de la app): además
  de modo Selected, marca las playlists propias que ya existían (si no, podrían quedar como
  no-held y podarse).
- Test nuevo `engine::tests::a_device_only_hands_on_the_playlists_it_holds` (el escenario del
  usuario). Verificado: `cargo test -p sway-core -p sway-server`, `cargo test --lib` en app.
  Sin probar en device.
- **Datos de prueba ya contaminados**: la PC dev y el server dev ya tienen esas playlists vacías y,
  como están en modo All, ahora las "tienen" y las seguirían pasando. Para probar limpio: borrar
  la DB de la PC dev (`<Music>\Sway Dev`) y la del server dev (`data/` en la raíz), re-emparejar.
- Limitación: sólo el lado que inicia el sync registra fuentes (el pasivo no recibe el manifest
  del otro). El celu inicia sus syncs (autosync/watcher), así que alcanza.
- Pendiente aparte (no hecho): server recibiendo el mismo archivo de dos devices a la vez → se sube
  dos veces y ambos escriben el mismo `<hash>.part` en `incoming` (colisión → hash mismatch y
  reintento). Propuesto: handshake "¿lo querés?" antes de mandar bytes + `.part` único por push.

## 2026-09-29 · Refresh = chequeo real + quitar/re-emparejar un server que nos soltó

- Reporte del usuario: tocar Refresh no actualizaba el estado ("Needs token" tras cambiar el
  token), y un server que ya nos soltó no se podía borrar (sólo decía "Needs token", sin botón).
- Causa: `refresh_peers` sólo re-preguntaba mDNS y hacía un probe TCP que **salteaba** los devices
  con watcher; nada abría sesión, así que un `NotPaired`/clave cambiada sólo se enteraba cuando el
  watcher reconectaba. Y `unpair` sólo sacaba de la lista a devices con fila en `devices`.
- Fix: `lib.rs refresh_peers` ahora es `async` + `spawn_blocking(pairing::check_now)` y devuelve
  cuando terminó. `pairing.rs check_now`: mDNS + `discovery::probe_all` (sin saltear watchers,
  probes en paralelo) + `verify_pairing` en paralelo con cada emparejado online (Hello + Bye con
  `open_session_with` timeout 5 s → registra NotPaired/roto/inalcanzable) + `watchers.wake`.
  `unpair`: saca de la lista a cualquier device de plataforma server (esté o no en `devices`) y
  sólo notifica `Unpair` si estaba emparejado. UI (`Sync.tsx`): badge "Needs token" para server no
  emparejado, botones **Pair again** (precarga host/puerto, foco al token) y **Remove**; el botón
  Refresh queda en "Scanning…" hasta que vuelve el comando.
- Verificado: `cargo check`, `cargo test --lib` (57), `pnpm tsc --noEmit`. Sin probar a mano.

## 2026-09-29 · 2.3 extra: cambiar el token del server revoca pairings + fix `pnpm server:dev`

- Reporte del usuario: `pnpm server:dev` creaba un archivo llamado `--config` en la raíz, y
  cambiar `pair_token` no desconectaba al celu.
- `--config`: el script pasaba `-- --config config.toml` y el binario toma el path como 1er
  argumento posicional → escribía un config nuevo en `./--config`. Fix: `package.json`
  `server:dev` = `cargo run -p sway-server -- config.toml`; `crates/server/src/main.rs
  config_path` ahora acepta `--config <path>` y sale con el uso ante otro flag. El `--config` que
  había se renombró a `config.toml` (gitignoreado; `data/` y `music/` en la raíz son de ese
  server de dev).
- Token (decisión del usuario: revocar): columna nueva `devices.token_hash` (`crates/core/src/db.rs`,
  schema + migración). `core/pairing.rs` `set_device_token` / `token_still_valid` (blake3
  `derive_key`, comparación con `secret_eq`). `crates/server/src/serve.rs`: al emparejar guarda el
  hash; en cada `Hello` de un device Trusted, si el token no coincide → borra la fila, log
  `paired with a previous token: unpaired`, contesta `NotPaired` → la app ya olvidaba el device por
  ese camino (`open_session_with`). **Los devices emparejados antes de esto no tienen hash → quedan
  revocados al actualizar el server: hay que re-emparejar una vez.** El token se lee al arrancar:
  cambiarlo requiere reiniciar el server (eso además corta los Watch abiertos).
- Docs: comentario del config generado (`config.rs`) y `crates/server/README.md`.
- Verificado: `cargo test -p sway-core -p sway-server` (tests nuevos
  `a_new_token_revokes_old_pairings` en `crates/server/tests/pairing.rs` y
  `a_new_token_invalidates_old_pairings` en core). Sin probar a mano.

## 2026-09-29 · Dev de Windows aislado del release (pedido suelto, fuera del plan)

- Sin commit. Archivo: `app/src-tauri/src/lib.rs`.
- Con `debug_assertions` (o sea `pnpm tauri dev`) en desktop:
  - Carpeta de datos `<Music>\Sway Dev` en vez de `<Music>\Sway` (const `DATA_DIR` en `lib.rs`):
    db, `library`, `covers`, `perf.log` y perfil de WebView2 (`webview`, o sea localStorage) todo
    aparte. Base nueva → uid de dispositivo nuevo.
  - Identifier `com.locardium.sway.dev` (fn `context()` en `lib.rs`, `config_mut()`): el plugin
    single-instance arma su mutex con el identifier; sin esto abrir dev con el release abierto
    sólo enfocaba la ventana del release y salía.
  - Nombre de dispositivo por defecto `<COMPUTERNAME> Dev` (se setea `SWAY_DEVICE_NAME` antes de
    abrir la db; sólo aplica cuando la db es nueva o el nombre es placeholder).
  - Título de ventana "Sway Dev".
- XML de iTunes NO se aísla (decisión del usuario): dev escribe el mismo
  `<Music>\iTunes\iTunes Music Library.xml` que el release si tiene auto-sync prendido.
- Dev y release se siguen viendo por mDNS (mismo service type): aparecen como dos dispositivos
  distintos y se pueden emparejar entre sí para probar sync. No se aisló a propósito.
- Verificado: `cargo check` limpio. Sin probar corriendo.

## 2026-09-29 · Fase 2 (2.1-2.4) — estado de conexión, pairing roto, un sync a la vez

- Sin commit (el usuario commitea cuando pruebe). **Protocolo cambiado sin compat**: nuevo
  `Msg::Busy` en `crates/core/src/wire.rs` y heartbeat del watch 45 s → 25 s → PC, celu y server
  tienen que ir en la misma versión (el server también: re-deployar `sway-server`).
- **2.1** (server caído sigue verde): `pairing.rs open_session_with` marca `mark_unreachable` +
  `peers-changed` cuando falla el connect (el watcher reintenta a los 3 s de un corte, así que un
  blip no se ve y un server caído se pinta gris en segundos). `watch.rs READ_TIMEOUT` 120 → 60 s y
  `engine.rs WATCH_HEARTBEAT` 45 → 25 s: un server que muere sin cerrar el socket (corte de luz/red)
  se detecta en ≤ ~65 s. `Drop for Live` sigue sin marcar offline (a propósito, ver comentario).
- **2.2** (aparece conectado antes de tiempo): `discovery.rs` — `Peer` tiene `checking: bool` e
  `invalid: Option<String>`. Un peer de mDNS nace `online: false, checking: true` y se proba al
  toque (`probe_uid` en un thread); el catch-up (`peer_came_online`) ahora sale sólo de
  `apply_probe` en la transición a online (antes salía del anuncio mDNS). `upsert` conserva lo que
  sabía el probe. `spawn_prober` proba en t=0 (antes esperaba 10 s) y emite `peers-changed` al
  cerrar la ventana de arranque (`DISCOVERY_WINDOW` 6 s, durante la cual los emparejados ausentes
  salen "checking"). UI: badge "Checking…" en `Sync.tsx` (`peerBadge`).
- **2.3** (pairing viejo no se olvida): la causa real que se ve en uso es **reinstalar/borrar la
  DB del server** → uid y clave nuevos; la fila vieja queda con la misma dirección, el probe la
  pinta verde (el puerto responde) y watcher/autosync reintentan para siempre contra un
  `KeyMismatch`. Rotar el token NO rompe pairings (el token sólo se chequea en `PairRequest`; los
  pairings son por clave) — no es bug. Fix: `pair_with_server` borra las filas de otros uids con la
  misma dirección (`[pair] ... was the previous identity`); `KeyMismatch` o `Reject` al abrir sesión
  → `Peers::set_invalid` (en memoria, se re-detecta en cada arranque) + `PairingBroken` tipado;
  watcher (`Outcome::Broken`, nap 1 h, lo despierta `forget_device`) y autosync (`invalid.is_none()`,
  `peer_came_online`) lo dejan en paz; `sync-done` automático no tira banner. UI: badge rojo
  "Pairing broken" + motivo + botón "Forget" (usa `unpairDevice`). `forget_device`/`store_device`
  limpian la marca. El `sync_log` de key-mismatch se escribe una vez por marca, no por reintento.
- **2.4** (celu recibe del server y de la PC a la vez): `pairing.rs` — `Pairing.active` pasó a
  `Slots { active, running }` + `Condvar`: **un solo sync corriendo por device, en cualquier
  dirección**. Saliente (`SyncGuard::acquire`) espera su turno (máx 30 min); entrante
  (`serve` → `SyncGuard::try_serve`) no espera: contesta `Msg::Busy` y el que llama
  (`run_sync_blocking`) suelta su slot, espera 3→30 s + jitter y reintenta (se rinde a los 20 min).
  `autosync::lan_peers` llama `discovery::wait_settled` (espera que la ventana de arranque cierre y
  no quede nadie "checking", máx 12 s) → el catch-up del server ve a la PC antes de decidir.
- Verificado: `cargo test -p sway-core -p sway-server` y `cargo test --lib` en `app/src-tauri`
  (57, con 4 tests nuevos en discovery), `pnpm tsc --noEmit`. **Sin probar en device.**
- Gotcha: `sed -i` en Git Bash deja CRLF → LF (pasó con `pairing.rs`); usar un script que preserve
  EOL y chequear `git ls-files --eol` antes de commitear.

## 2026-09-29 · Limpieza de ramas de git

- Pedido del usuario: entender las ramas, borrar/unir lo que sobre y dejar todo commiteado.
- Estado encontrado: `master` == `origin/master` == `beta1-fase1-3` (2cc0762, las fases 1+3 ya
  estaban pusheadas); `beta1-fase4` = master + 1 commit (93e7833); `backup-pre-rewrite`
  (c3a680a, 2026-08-21) = historia vieja anterior a una reescritura, sin ancestro común con
  master, pero su árbol es **idéntico** al de `df8a67b` en master; stash `pre-rewrite wip`
  (9188120, 2026-08-25) = versiones viejas de TrackTable/lib.rs/styles.css ya superadas en
  `6f462b9` y posteriores.
- Hecho: fast-forward de `master` a `beta1-fase4`; borradas `beta1-fase1-3`, `beta1-fase4`,
  `backup-pre-rewrite` (con OK del usuario) y el stash (con OK). Recuperables un tiempo por reflog
  / `git fsck --unreachable` con esos hashes. Queda sólo `master`, adelante de origin. Sin push.

## 2026-09-29 · Fase 4 cerrada y commiteada

- El usuario probó todo en desktop y confirmó que anda (4.1-4.6, incluida la poda de carátulas).
- Rama `beta1-fase4` (desde `master` == `beta1-fase1-3`), un commit con todo el código de la
  Fase 4 + `docs/progress`. Antes se restauró `gen/android/app/build.gradle.kts` con
  `git checkout`: el CLI de Tauri (`pnpm dev:android`) reescribe la línea `applicationIdSuffix`
  en LF y el archivo queda `w/mixed` sin cambio de contenido — pasa en cada corrida, revisarlo
  antes de cada commit. Sin push.

## 2026-09-25 · 4.6 Menú contextual del sidebar se corta abajo (reporte del usuario)

- Síntoma: click derecho en una playlist cerca del borde inferior → el menú se abre hacia abajo y
  queda cortado (sólo se ve "Rename").
- Causa: `components/Sidebar.tsx` tenía su propio `<div className="ctx-menu">` en `left/top` del
  cursor, sin ningún ajuste al viewport (la tabla sí usaba `ContextMenu`, que clampeaba).
- Fix: Sidebar usa `ContextMenu` (items como `MenuItem[]`, `key` por posición para re-medir).
  `ContextMenu.tsx`: en vez de clampear, hace flip como un menú nativo (arriba/izquierda del
  cursor si no entra abajo/derecha, mínimo 8 px del borde) y se renderiza con `createPortal` a
  `document.body` (dentro del drawer del sidebar en mobile, que tiene `transform`, el
  `position: fixed` quedaba relativo al drawer y recortado).
- Usuario sobre 4.5: "por ahora dejalo así" (le gusta pero lo ve raro; se le ofreció cambiar
  `overflow-wrap: anywhere` → `break-word`, no respondió todavía).
- Verificado: `pnpm tsc --noEmit` ok. Sin probar en la app.

## 2026-09-25 · Prueba de Fase 4 en desktop + carátulas de temas borrados

- Usuario confirmó: Reveal anda, drag hacia adentro/afuera y al Explorer anda, scroll de
  carátulas anda. 4.5: "lo veo igual" — esperable: el apilado es `max-width: 760px` y
  `min_inner_size` de la ventana es 720, así que en desktop sólo se nota el título en 2 líneas;
  el cambio grande es en el celu. Pendiente que decida si queda. 4.1 sin probar.
- Pedido: al borrar un tema, borrar su carátula. `app/src-tauri/src/cover.rs`: `prune()` corre
  antes de `warm_missing` en el thread de `Covers::warm` — borra de `covers/` todo archivo cuyo
  stem (hash) no tenga ningún track (incluye `.tmp` viejos), y saca de `memory` los ids que ya no
  existen. Si la query falla no poda (una lista vacía borraría todo). `lib.rs` `delete_tracks`
  llama `kick_background`. Cubre también borrados por sync/watcher en el próximo kick.
- Hallazgo: `tracks.id` es `INTEGER PRIMARY KEY` sin AUTOINCREMENT → SQLite reusa el id más alto
  borrado; un tema nuevo heredaba la carátula cacheada por id. Por eso además: `Cover.tsx`
  exporta `keepCovers(ids)` y `App.tsx` `refreshLibrary` lo llama con los ids de la librería.
- Verificado: `pnpm tsc --noEmit`, `cargo check` ok. Sin probar en la app.

## 2026-09-25 · Fase 4 completa en código (4.1-4.5), sin commit, sin probar en desktop real

- **4.3** `app/src-tauri/src/lib.rs` `reveal_track`: en Windows `.raw_arg(format!("/select,\"{path}\""))`
  (con `.arg()` Rust envolvía todo en comillas si la ruta tenía espacios y explorer abría una
  ventana genérica; sin espacios andaba). Barras `/` → `\`. Linux: `dbus-send ...
  FileManager1.ShowItems` en un thread, fallback `xdg-open` de la carpeta; las comas del URI van
  como `%2C` porque `array:string:` de dbus-send separa por coma. Lee con `db_read`.
- **4.5** `app/src/styles.css`: columna now-playing `minmax(180px, 1fr)`; título con
  `-webkit-line-clamp: 2`, artista 1 línea; `@media (max-width: 760px)` apila el player en 2 filas
  (`"np right" / "center center"`) y sube `--player-clear` a 148 px (las listas reservan ese
  espacio abajo). Aplica también al celu (ancho < 760).
- **4.1** `app/src/App.tsx`: `plTracks` ahora es derivado de `pl: {id, tracks}` (vacío si `pl.id`
  no es la selección actual). `refreshPlaylistTracks` es estable (deps `[]`), lee la selección de
  `selectionRef` y sólo escribe si es el último request (`plRequest`). El effect de selección lo
  usa (también invalida requests viejos al ir a Library). Todos los callers (drop del OS, reorder,
  remove, delete, library-changed...) pasan por ahí, así que no hizo falta tocar cada línea que
  listaba el plan.
- **4.4** `app/src/dnd.ts`: `resolveTarget` salió de `beginDrag` como `targetAt(x, y, kind)`
  exportado; nuevo handler opcional `onExit` que corta el drag interno cuando el mouse sale de la
  ventana. `App.tsx`: `osHoverRef` muestra el indicador `dropInsertIndex` durante el drag del OS;
  `osDropRef` calcula el índice y después de `addTracksToPlaylist` llama `reorderPlaylistTracks`
  (el índice es sobre la lista completa y el backend descuenta los movidos que estaban antes, así
  que el índice calculado antes del drop sirve). Posición: `p.position.toLogical(devicePixelRatio)`
  (dpr incluye escala del monitor y zoom de página = píxeles CSS). Hacia afuera: comando
  `drag_tracks_out(ids)` en `lib.rs` con el crate **`drag` 2.1** directo (no `tauri-plugin-drag`:
  sin paquete JS ni capability; resuelve rutas en Rust). Corre `drag::start_drag` en el main
  thread (DoDragDrop es modal); imagen = thumb de carátula en disco (`Covers::file`, WIC decodifica
  JPEG) o `icons/64x64.png`. Dep sólo desktop en `app/src-tauri/Cargo.toml`. `api.ts`: `dragTracksOut`.
- **4.2** `app/src/components/Cover.tsx`: cola global (máx 4 `cover_thumb` en vuelo, LIFO = la
  fila que se ve ahora primero, se cancela si la fila sale del margen antes de su turno) y un
  `IntersectionObserver` por contenedor con `root = .table-wrap` y `rootMargin: 600px 0px`
  (con root = viewport el overflow de la tabla recorta y el margen no sirve). Observer con
  refcount: se desconecta cuando la tabla se desmonta. Las `eager` (player/panel) usan la misma
  cola (entran primeras por LIFO).
- Verificado: `pnpm tsc --noEmit` ok, `cargo check` desktop ok, `cargo check --target
  aarch64-linux-android` ok sin warnings (con `CC_aarch64_linux_android`/`AR_...` del NDK 28 y
  `CARGO_TARGET_DIR=target/android-check`). EOL: check de STATUS vacío.
- **Falta probar en desktop (usuario):** (1) Reveal con ruta con espacios; (2) achicar la ventana
  < 760 px: el player apila, título visible; (3) soltar archivos en una playlist y cambiar a otra
  antes de que termine el import: no deben aparecer ahí; (4) soltar desde Explorer entre dos filas
  → línea indicadora y los temas quedan ahí; (5) arrastrar un tema fuera de la ventana y soltarlo
  en el Explorer/escritorio (copia) — riesgo: que WebView2 quede creyendo que el botón sigue
  apretado tras el DoDragDrop; (6) scroll rápido en playlist grande.

## 2026-09-25 · Commit de todo lo hecho (fases 1 y 3) + pase a Fase 4

- Pedido del usuario: commitear todo y seguir con la Fase 4 en otro chat.
- Rama `beta1-fase1-3` (desde `master`), dos commits: (1) identificador `com.locardium.sway` +
  dev/release separadas (cambios previos al plan: `.gitignore`, `tauri.conf.json`,
  `build.gradle.kts`, manifest, rename de `MainActivity.kt`, `CLAUDE.md`); (2) todo el trabajo del
  plan + `docs/progress`. `lib.rs` mezcla ambos temas y va entero en el (2).
- Antes de commitear se restauró CRLF en 13 archivos que estaban en LF en el working tree (el
  diff pasó de ~15.000 a ~2.900 líneas reales). Verificado después: `tsc` ok, `cargo check` ok,
  `cargo test -p sway-core` 130/130.
- Sin push.

## 2026-09-25 · Fase 3 confirmada

- El usuario respondió "listo genial" tras las instrucciones de prueba de la Fase 3 (+ tema con
  `#`). Marcado ✅. Sigue: Fase 2 o Fase 4. Sin commit.

## 2026-09-25 · Fase 3 (3.1 normalize, 3.2 carátula, 3.3 stop) + tema con `#` que no sonaba

- Detalle y decisiones en PLAN 3.1/3.2/3.3. Archivos: `crates/core/src/db.rs` (TARGET_LUFS,
  peak_db, playback_gain_db, tests), `crates/core/src/engine.rs` (test), `app/src-tauri/src/
  loudness.rs`, `lib.rs` (TrackCue.artwork, cues), `cover.rs` (`Covers::file`),
  `NativeAudioPlugin.kt` (fileUri, artwork, stop, gain al tema actual en setQueue),
  `NativeAudioService.kt` (carátula, stop action), `crates/native-audio/build.rs` +
  permissions (`stop`), `app/src/nativeAudio.ts`, `nativeAudioPlugin.ts`, `App.tsx`, `api.ts`.
- Verificado: `cargo test -p sway-core` **130/130** (primera vez en verde), `sway-server` ok,
  `cargo check` app ok, `tsc` ok. Kotlin sin compilar (el usuario corre `pnpm dev:android`).
  Nada probado en device. Sin commit.
- Gotcha: al agregar un comando al plugin, `touch crates/native-audio/build.rs` + `cargo check -p
  tauri-plugin-native-audio` para que regenere `permissions/autogenerated/commands/<cmd>.toml`.

## 2026-09-25 · Barra de sync = tanda completa + carátulas en disco (4.2)

- Barra: el usuario quiere en la PC lo mismo que Android (tema N de M, barra por la tanda). El que
  inicia manda `index`/`of` en `BlobReq` y `BlobPush` (`wire.rs`, campos OBLIGATORIOS: el usuario
  dijo que no importa la compatibilidad con versiones viejas y pidió el código más limpio → una
  PC/celu nuevo NO sincroniza con uno viejo). `transfer::pull_file` recibe `(index, of)`; se
  sacaron todas las ramas de "total desconocido" (engine, sync_notice, UI). `api.ts
  syncRunPercent` compartido por el panel Sync y el toast.
- Carátulas: ver PLAN 4.2. Archivos: `cover.rs`, `lib.rs` (`kick_background`, `cover_thumb`,
  `covers_dir`), `pairing.rs`.
- Verificado: `cargo test -p sway-core` 125 ok + 4 preexistentes, `sway-server` ok, `cargo check`
  app ok, `tsc` ok. Sin probar en device. Sin commit.

## 2026-09-25 · Barra de progreso del lado que ENVÍA (PC)

- Usuario: la barra visual no sirve en la PC. Causa: `transfer::send_file` nunca reportó bytes;
  el lado que envía emitía un solo `Progress` con `done: 0` por archivo (push de `engine::sync`)
  o `total: 0` (lado que sirve, `BlobReq`). La barra del panel Sync al ENVIAR estaba en 0 desde
  siempre; el toast nuevo también cuando la PC sirve.
- Fix: `send_file(..., progress: &mut dyn FnMut(u64, u64))` llamado por chunk; callers en
  `engine.rs` (push y `BlobReq`) pasan el closure; tests de `transfer.rs` pasan `&mut |_, _| {}`.
  `SyncIndicator.tsx`: la barra avanza también dentro de cada archivo.
- Verificado: `cargo test -p sway-core` 125 ok + 4 preexistentes, `sway-server` ok, `cargo check`
  app ok, `tsc` ok. Sin probar. El build Android de validación de la notificación de análisis lo
  cortó el usuario (no relanzar desde Claude si él está corriendo `pnpm dev:android`). Sin commit.

## 2026-09-25 · Notificación también para el análisis de loudness (Rescan / temas nuevos)

- Pedido del usuario. Archivos: `app/src-tauri/src/sync_notice.rs` (`analysis`, `analysis_end`,
  prioridad del sync), `loudness.rs`, `crates/native-audio/src/lib.rs` (`icon` en vez de
  `sending`), `NativeAudioPlugin.kt`, `SyncService.kt`.
- Verificado: el build APK release de la tanda anterior (notificación de sync) compiló OK;
  `cargo check` desktop OK con esto; build Android de validación relanzado. Sin probar en device.
  Sin commit.

## 2026-09-25 · 1.7A confirmado + 1.2/1.3/1.4: notificación de sync (FGS dataSync) y lag

- 1.7A: el usuario lo probó y anda; pidió no seguir probándolo desde Claude. Gotcha: `adb shell am
  kill` NO mata la app mientras tiene sesión de medios; `adb shell run-as <pkg> kill -9 <pid>`
  sí (sólo builds debuggables) — pero la prueba interrumpió al usuario: preguntar antes.
- Pedido del usuario: notificación con el progreso del sync para que Android no mate el proceso.
- Archivos: `crates/native-audio/android/.../SyncService.kt` (nuevo), `NativeAudioPlugin.kt`
  (comandos `showSyncNotice`/`hideSyncNotice`, tick 100 ms), manifest del plugin,
  `crates/native-audio/src/lib.rs` + `Cargo.toml` (serde, serde_json, log),
  `app/src-tauri/src/sync_notice.rs` (nuevo), `lib.rs`, `pairing.rs`, `loudness.rs`,
  `crates/core/src/engine.rs` (progreso del lado que sirve), `app/src/components/Sync.tsx` y
  `SyncIndicator.tsx` (`sync-idle`, total desconocido). Detalle en PLAN 1.2/1.3/1.4.
- Verificado: `cargo check` app OK, `pnpm tsc --noEmit` OK; build APK release de validación
  (ver entrada siguiente o STATUS). Sin probar en device. Sin commit.

## 2026-09-25 · 1.8 confirmado

- El usuario probó con builds nuevos (celu dev + PC): conteo de playlists e indicador de sync
  funcionan. 1.8 y 1.9 cerrados. Sin commit.

## 2026-09-25 · 1.8: conteo real de playlists no descargadas (`parked_memberships`)

- El usuario eligió la variante simple: "que muestre cuántos temas tendría cada playlist".
- Archivos: `crates/core/src/db.rs` (tabla en `migrate`, query de `list_playlists`),
  `crates/core/src/merge.rs` (estacionar/limpiar en memberships y `apply_tombstone`, + test).
- Verificado: `cargo test -p sway-core` 125 ok + las 4 fallas preexistentes (3 normalización, 1
  engine); `cargo test -p sway-server` ok; `cargo check` de la app ok.
- Gotcha: `db::tests::migrate_backfills_ranks_preserving_old_order` llama solo a `migrate` sobre un
  esquema viejo: una tabla nueva que se lee en `list_playlists` tiene que crearse en `migrate`,
  no solo en SCHEMA.
- Sin probar en device. Sin commit.

## 2026-09-25 · 1.8: contador 0 (diagnóstico) + indicador de sync global

- Usuario: tras vincular, las playlists aparecen y sincroniza bien. Reporta (a) playlists no
  marcadas con 0 temas en el celu y (b) que no se ve en ningún lado cómo pasan los temas.
- (a) Diagnóstico por IPC en el celu: 128 tracks (la playlist marcada), 26 playlists, todas las no
  marcadas con trackCount 0. Causa de modelo (plan 1.8 "Hallazgo 2"). Sin tocar: decisión pendiente.
- (b) `sync-progress` se emitía siempre pero solo lo dibujaba el panel Sync. Nuevo
  `app/src/components/SyncIndicator.tsx` (toast abajo a la derecha, repinta cada 250 ms, se oculta
  con el panel Sync abierto / import en curso / 30 s sin noticias), montado en `App.tsx`; estilo
  `.sync-toast-file` en `styles.css`. `pnpm tsc --noEmit` OK. Sin probar en device. Sin commit.

## 2026-09-25 · 1.9 confirmado + 1.8: tras vincular no arrancaba ningún sync

- El usuario probó el pairing con el fix de 1.9: anda.
- Reportó: desde el celu no puede elegir playlists. Diagnóstico por IPC (DevTools del webview):
  `scope_prompt_pending=false` (ya había elegido "Choose playlists"), scope "selected", 0 playlists,
  y logcat sin ningún `[sync]` después del pairing. Causa y fix en PLAN 1.8 ("Hallazgo en device"):
  `connect_inner` ahora llama `autosync::peer_came_online` tras el `Hello` del vínculo nuevo.
- Archivo: `app/src-tauri/src/pairing.rs`. `cargo check` OK. Sin probar en device. Sin commit.
- Ojo: el celu seguía con la dev de las 14:40 (pid viejo); para probar hay que reinstalar.

## 2026-09-25 · 1.9 pairing: código perdido + freeze al cerrar (investigado y arreglado)

- Reproducido con el MSI instalado en la PC (28/8) + la dev en el celu: NO es de la app dev ni de
  datos nuevos. La lógica de pairing no cambió desde el último commit.
- Método (útil para otros bugs de UI): DevTools remoto en los DOS webviews.
  - PC: relanzar el exe con `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222`
    (y `RUST_LOG=...` + `-RedirectStandardError` para los logs Rust: el exe no tiene consola).
  - Celu (build debuggable): `adb forward tcp:9223 localabstract:webview_devtools_remote_<pid>`.
  - Con un script node de 15 líneas (`Runtime.evaluate` por WebSocket) se lee el DOM, se hace
    click y se registran eventos Tauri con
    `__TAURI_INTERNALS__.invoke('plugin:event|listen', {event, target:{kind:'Any'}, handler:
    __TAURI_INTERNALS__.transformCallback(fn)})`.
- Causas (detalle en PLAN 1.9): (1) `pairing-request` solo lo escuchaba `Sync.tsx`; (2) colisión en
  `Pairing.pending` por uid al expirar un pedido no visto; (3) el "freeze" = `.modal-backdrop
  .closing` invisible que se come los toques, porque Sync devolvía el Modal del código en el mismo
  lugar que su Modal y React reusaba la instancia con `closing=true`; (4) el receptor no veía la
  cancelación del iniciador. La hipótesis "comando síncrono bloquea el main thread" era falsa.
- Archivos: `app/src/components/PairingDialog.tsx` (nuevo), `app/src/App.tsx`,
  `app/src/components/Sync.tsx`, `app/src/styles.css`, `app/src-tauri/src/pairing.rs`,
  `crates/core/src/wire.rs` (+ test).
- Verificado: `cargo check` (app) OK, `pnpm tsc --noEmit` OK, test nuevo de `peer_has_spoken` OK.
  Falta en device (ver STATUS).
- Sin commit.

## 2026-09-25 · Cierre del chat (continua en uno nuevo)

- Bug nuevo reportado al final, sin investigar: 1.9 (pairing, ver plan). Todo lo demas del dia esta
  en las entradas de abajo y en STATUS "Proximo paso".
- Gotcha: correr `pnpm dev:android` en background desde Claude deja un `vite` huerfano en los
  puertos 1420/1421 y la siguiente corrida del usuario falla con "Port 1420 is already in use".
  Buscarlo con `Get-NetTCPConnection -LocalPort 1420` y matar ese node.exe. Mejor que el usuario
  corra `pnpm dev:android` en su terminal.
- Nada commiteado en todo el dia (el usuario no lo pidio). Hay muchos archivos modificados;
  ademas de los cambios previos al plan (identificador `com.locardium.sway`, etc.).

## 2026-09-25 · 1.8 primer vinculo pregunta + scope re-chequeado en el sync + logs en Android

- Reporte del usuario (probando la app dev, primer vinculo con la PC): bajo toda la biblioteca
  aunque destildo playlists en el medio; logcat lleno de `symphonia_metadata::id3v2: unsupported
  frame GEOB/RVAD`. Tambien confirmo: **la notificacion "funciona perfecta"** con la cola nativa.
- Cambios: ver PLAN 1.8. Archivos: `app/src-tauri/src/pairing.rs` (`store_device`,
  `is_first_pairing`, `SCOPE_PROMPT_KEY`), `app/src-tauri/src/lib.rs` (comandos
  `scope_prompt_pending`/`resolve_scope_prompt`, `LOG_FILTER` compartido desktop/Android),
  `crates/core/src/manifest.rs` (`wanted_now` + test, `PartialEq`), `crates/core/src/engine.rs`
  (re-chequeo de scope entre archivos + log de omitidos), `app/src/api.ts`,
  `app/src/components/Sync.tsx` (tarjeta de la pregunta), `app/src/styles.css`.
- Verificado: `cargo check` OK, `pnpm tsc --noEmit` OK, `cargo test -p sway-core`: 124 ok,
  **4 fallas PREEXISTENTES** no relacionadas: 3 de `db::tests` de normalizacion (target LUFS,
  ver 3.1) y `engine::tests::editing_both_sides_without_seeing_each_other_loses_neither_change`
  (el test ordena la lista y la compara contra `["Two","One"]` sin ordenar — bug del test).
  APK release instalado 14:24:46 (tambien valida que el filtro de android_logger compila).
- Pendiente: probar en device el primer vinculo (con la app dev: desvincular y volver a vincular
  no alcanza si el dispositivo ya tiene fila en `device_sync`; con datos nuevos si).
- Sin commit.

## 2026-09-25 · Dev y release de Android como apps separadas

- Problema: el identificador cambio de `com.sway.app` a `com.locardium.sway` (cambio previo, sin
  commitear), asi que `pnpm dev:android` (firma debug) chocaba con la release instalada (firma
  Locardium): `INSTALL_FAILED_UPDATE_INCOMPATIBLE`. No borro nada (adb solo rechaza).
- Intento fallido: `applicationIdSuffix = ".dev"` a mano en `build.gradle.kts` → el CLI de Tauri
  2.11 lo borra en cada corrida (regex `^[ \t]*applicationIdSuffix\s*=.*` en el binario del CLI).
- Fix: `"debugApplicationIdSuffix": ".dev"` en `bundle.android` de `app/src-tauri/tauri.conf.json`
  (opcion oficial de Tauri para instalar debug y release lado a lado); el CLI escribe el sufijo.
  Ademas `manifestPlaceholders["appLabel"]` (release "@string/app_name", debug "Sway Dev") y el
  manifest usa `${appLabel}` en application y activity, para distinguirlas en el celu.
- Verificado: `pnpm dev:android` instala y abre `com.locardium.sway.dev`; `com.locardium.sway`
  (release, 12:51:39) sigue igual. La dev arranca con biblioteca vacia (datos propios).
- La app `com.sway.app` (0.1.0) es la dev vieja de antes del rename; se puede desinstalar.
- Regla agregada al CLAUDE.md.

## 2026-09-25 · 1.7 parte A: cola nativa (instalada en el S24, falta prueba del usuario)

- Kotlin (`crates/native-audio/.../NativeAudioPlugin.kt`): cola en `NativeAudioRuntime`
  (`queue: List<SourceSpec>`, `queuePos`, `repeatMode`, `autoplay`); comandos `set_queue`,
  `set_queue_mode`, `queue_next`, `queue_prev`, `restore_queue` (registrados en `build.rs` +
  `permissions/default.toml`, autogenerados regenerados con cargo check). Pase automatico: hooks en
  `onMediaItemTransition` (gapless, restage posteado porque tocar el playlist dentro del callback
  re-entra en ExoPlayer) y en `startFadeLocked` (crossfade, restage via `pendingNext`).
  Persistencia: items en `filesDir/native_audio_queue.json` (escrito en `ioHandler`, tmp+rename),
  pos/repeat/autoplay en prefs `tauri_native_audio_queue`. `restoreQueue` lee en `ioHandler` y
  aplica en el main looper; en proceso nuevo carga la entrada actual pausada en el checkpoint.
  `transport()` y el broadcast de `MainActivity` van primero a `mediaButton()` (cola nativa) y
  solo sin cola caen al camino viejo por JS. `setNextSource` se ignora con cola. `setSource`
  "a la antigua" alinea `queuePos` si el id esta en la cola (si no, limpia la cola).
  `setSourceGain` actualiza la copia del gain en la cola. `SourceSpec` paso a `internal`.
- JS: `nativeAudioPlugin.ts` (wrappers + `repeatMode/queueIndex/queueLength` en el estado),
  `nativeAudio.ts` (`playQueue`/`updateQueue`/`setQueueMode`/`nextTrack`/`prevTrack`; restore
  llama `restoreQueue` primero; stop borra la cola nativa; evento `repeat`; sin el hack
  `NEAR_END_LEAD_MS` cuando hay cola nativa), `api.ts` (`nativeQueue`, null en desktop),
  `App.tsx` (en Android: onPlay/playOffset/onPrev/shuffle/modos → cola nativa; onTrackEnded solo
  pausa; onAdvanced no re-encola; prefs → refresca cues; restore le da la cola de JS al player si
  no trajo una propia). Desktop sin cambios de comportamiento.
- Verificado: `pnpm tsc --noEmit` OK, `cargo check` OK, `gradlew compileDebugKotlin` (plugin +
  app) OK, APK arm64 instalado 12:51:39. Logcat grabandose en `scratchpad/test-17a.log`.
- Pendiente parte B: reanudacion Media3 para botones con el proceso muerto sin abrir la app.
- Sin commit.

## 2026-09-25 · 1.7 (parte Rust) + 3.2 adelantado + causa confirmada de 1.6

- **Causa de 1.6 confirmada** con `adb shell dumpsys activity exit-info com.locardium.sway`
  (historial de muertes del proceso, lo guarda Android, no hace falta reproducir): ~15 muertes en
  una semana, todas con `importance=400` (cached = musica pausada, servicio fuera de foreground).
  Razones: `AL_Kill : over 1 slots` (limitador de apps cacheadas de Samsung, la mas comun),
  `LOW_MEMORY`, `KILL BACKGROUND` 03:39 (optimizacion nocturna Samsung), `REMOVE TASK` (swipe).
  Al revivir, el restore de 1.5 solo recupera el tema del checkpoint → "vuelve a la cancion donde
  se quedo y se olvida la cola". Detalle y correcciones en PLAN 1.6.
- **Sintoma nuevo reportado por el usuario:** "la notificacion queda bugueada y siempre que
  intento adelantar vuelve a la ultima cancion". **Reproducido en vivo con el usuario (11:49-11:56,
  logcat en `scratchpad/repro-notif.log`) y explicado:** NO es la sesion enganchada a otro
  reproductor (hipotesis descartada: una sola sesion, una sola notificacion, mismo pid) ni muerte
  del proceso. Es React sin re-renderizar en background: los next llegan a JS pero usan un
  `currentId` viejo (refs que solo se actualizan en el render), y cada uno recarga el mismo tema
  desde 0. Detalle en PLAN 1.6 ("segundo mecanismo").
- **1.7 parte Rust (hecha):** `app/src-tauri/src/lib.rs`: helper `cue_for_track` compartido;
  `track_cue` ahora usa `db_read` (antes tomaba el lock de escritura para solo leer); comando
  nuevo `queue_cues(ids) -> Vec<TrackCue>` (una pasada por la DB, omite ids que no existen);
  `TrackCue` suma `id`, `title`, `artist`. Registrado en `generate_handler!`. Se descarto el
  puente JNI (ver plan 1.7) y se saco `jni` del `Cargo.toml`.
- **3.2 adelantado (barato):** `app/src/nativeAudio.ts` pasa `title`/`artist` del cue a
  `setSource`/`setNextSource`/restore → la notificacion deberia mostrar el nombre aunque el archivo
  no tenga tags. Falta la caratula (artworkUrl) — sigue pendiente dentro de 3.2.
- Verificado: `cargo check -p sway` OK, `pnpm tsc --noEmit` OK. Sin build/APK nuevo todavia
  (el celu tiene el APK de las 01:37, que se esta usando para la prueba de la notificacion).
- Sin commit.

## 2026-09-25 · Bug nuevo en device: next/prev por notificacion/Bluetooth se cuelga + 1.3 (parcial)

- **Reporte del usuario** (14 dias usando el build de 1.1+1.5+1.0 en su S24, sin crashes ni
  freezes): "por ahi quiero cambiar de cancion por notificacion o bluetooth (radio del auto) y
  cambia... pero la notificacion vuelve a una ultima cancion que se acuerda y no avanza, tengo
  que abrir la app y cambiar manualmente."
- **Causa (por lectura de codigo, NO reproducida en device todavia):**
  `MainActivity.dispatchMediaButton` solo puede llegar a JS por `live` (referencia a la Activity
  viva) -> `activity.webView` -> `evaluateJavascript`. El servicio de audio
  (`NativeAudioService : MediaSessionService`) es un componente separado que sobrevive a que
  Android destruya la Activity para liberar memoria (pasa sin avisar, con la musica sonando
  igual). Con la Activity muerta, `dispatchMediaButton` hacia `return` en silencio: el boton no
  hacia nada, ni siquiera un fallback. Como JS solo pre-encola UN tema siguiente
  (`setNextTrack(followerOf(id))` cada vez que arranca un tema), el primer "next" que llega
  mientras la Activity todavia esta viva alcanza a funcionar (cambia de tema Y pre-encola el
  proximo), pero apenas la Activity muere, el siguiente "next" no tiene a donde ir del lado
  nativo tampoco (nunca existia un fallback) -> coincide exacto con "cambia una vez y despues no
  avanza mas".
- **Fix (implementado, NO probado en device todavia):**
  - `crates/native-audio/android/.../NativeAudioPlugin.kt`: `transportInterceptor` paso de
    `(String) -> Unit` a `(String) -> Boolean` (si `false`, `transport()` cae a un fallback
    nativo: "next" usa `skipToNext(appContext)` -- la funcion que YA existia para arrancar el
    tema pre-encolado sin pasar por JS --, "prev" hace `seekTo(0)` sobre el deck activo, restart
    del tema).
  - `MainActivity.kt`: `dispatchMediaButton` ahora devuelve `Boolean` (si `live`/`webView`
    estaban vivos).
  - Limite conocido: sin JS vivo solo hay UN paso nativo disponible para "next" (lo ultimo que JS
    pre-encolo); despues de usarlo, sin nada mas pre-encolado, el "next" siguiente vuelve a no
    hacer nada hasta reabrir la app. Mejor que "nunca funciona", no es el fix completo (mover
    queue/shuffle/repeat a Kotlin/Rust queda fuera de alcance por ahora).
- **1.3 parcial, adelantado por esto:** se implemento la exencion de "ignorar optimizacion de
  bateria" pedida por el usuario el 2026-09-11 (quedaba pendiente dentro de 1.3), porque
  probablemente es lo que esta matando/congelando el proceso en primer lugar (Doze / Samsung
  "sleeping apps"). Nuevo: `is_ignoring_battery_optimizations` / `request_ignore_battery_optimizations`
  como comandos del plugin `native-audio` (no es tematicamente audio, pero es el unico plugin
  Android que Sway tiene armado con el puente JS-Kotlin de Tauri). Permiso
  `REQUEST_IGNORE_BATTERY_OPTIMIZATIONS` en el manifest de la app. Toggle en Settings.tsx (solo
  Android), re-chequea el estado con el evento `focus` de la ventana (el dialogo del sistema es
  otra Activity encima). **Falta:** el resto de 1.3 (foreground service dedicado para el sync con
  pantalla apagada, WifiLock) sigue sin hacer -- esto solo cubre la exencion de bateria.
- Archivos: `crates/native-audio/build.rs` (2 comandos nuevos en `COMMANDS`),
  `crates/native-audio/permissions/default.toml` (+2 `allow-*`, regenerados con
  `cargo check -p tauri-plugin-native-audio` los `autogenerated/*`),
  `crates/native-audio/android/.../NativeAudioPlugin.kt`,
  `app/src-tauri/gen/android/app/src/main/java/com/locardium/sway/MainActivity.kt`,
  `app/src-tauri/gen/android/app/src/main/AndroidManifest.xml`,
  `app/src/nativeAudioPlugin.ts`, `app/src/nativeAudio.ts`, `app/src/api.ts`,
  `app/src/components/Settings.tsx`.
- Verificado: `pnpm tsc --noEmit` OK, `cargo check -p tauri-plugin-native-audio` OK (regenero
  permisos), `cargo check -p sway` OK, `gradlew :tauri-plugin-native-audio:compileDebugKotlin
  :app:compileArmDebugKotlin` OK. **NO probado en el celular todavia** -- falta build+install+la
  prueba real (dejar la app en background mucho rato con musica sonando, usar Bluetooth/auto para
  next/prev, ver si sigue respondiendo).
- Sin commit.

## 2026-09-11 · 1.0 Loop de reimport del watcher (hallazgo en device)

- Síntoma: con el APK de 1.1+1.5 instalado, abrir la app y cambiar de playlist colgaba la UI.
- Diagnóstico por adb (Tailscale): no hubo crash (el proceso murió por "remove task" = el usuario lo
  cerró desde recientes). `perf.log` (en `Android/data/com.locardium.sway/files/Music/`, se puede
  `adb pull`) mostró `LOCK held ~16 s by lib.rs:1640`. logcat: los 926 archivos abiertos por lofty
  2-4 veces en 44 s; `io_stats` ~3 GB leídos cada 4 s; WAL de 6 MB sin imports reales.
- Causa: notify 8.2.0 vigila OPEN/CLOSE_NOWRITE → evento con path = carpeta Music → pasa el filtro
  por path → `import_paths` expande el dir → reimport + rehash de toda la librería con el lock
  de escritura → el walk reabre la carpeta → loop. Bug previo a los cambios de hoy.
- Fix: `app/src-tauri/src/lib.rs` `spawn_folder_watch`: `.filter(|e| !e.path.is_dir())`.
- Verificado en device (APK arm64 instalado 22:57:14, `logcat -c` + abrir app + 30 s): lofty
  23 aperturas (antes 2128 en 44 s — ahora sólo carátulas visibles), `io_stats` 45 MB y después
  14 MB por ventana de 4 s (antes ~3 GB), `perf.log` sin bloqueos de lock nuevos.
- Gotchas útiles: `adb shell ls/pull` sobre `Android/data/com.locardium.sway/files/Music/` funciona
  (grupo ext_data_rw). Los `console.*` del WebView NO salen en logcat en build release.
- Versión: `tauri.conf.json`/`Cargo.toml` siguen en `1.0.0-1` (no se bumpeó); la forma de ver que
  se instaló es `dumpsys package com.locardium.sway | grep lastUpdateTime`. En el celu sigue
  instalada la app vieja `com.sway.app` (0.1.0, previa al rename de paquete).

## 2026-09-11 · 1.5 Volver a la app = estado perdido (🧪 falta device)

- `app/src/nativeAudio.ts`: `restorePlayback()` nuevo (adopta el track del plugin si sigue vivo;
  si no, carga el del checkpoint pausado en su posición). `restoreCancelled` lo cancela si el
  usuario hace play/stop antes. `stopPlayback` ahora llama `clearProgressCheckpoint`.
- `app/src/api.ts`: `restorePlayback` (null en desktop).
- `app/src/App.tsx`: effect de restore al montar; `setQueue()` persiste la cola en `ui_setting`
  `queue`; `selection` con `useUiSetting('selection')` (sólo Android, `writeDelayMs: 0`).
- `gen/android/.../AndroidManifest.xml`: `configChanges` ampliado.
- Sin commit.
- Verificado: `pnpm tsc --noEmit` OK. Permisos: `native-audio:default` ya incluía
  `get-progress-checkpoint`/`clear-progress-checkpoint`. Build: `pnpm build:android` (resultado abajo
  en STATUS/LOG si falló).
- Cómo probar en device: poner un tema, pausar, abrir 3 apps pesadas (o `adb shell am kill
  com.locardium.sway` con la app en background) y volver → mismo tema, misma posición, misma
  playlist, play retoma. Con música sonando: cambiar tamaño de letra del sistema → la app no se
  reinicia. Stop + cerrar + abrir → no vuelve el tema.
- Hallazgo: el caso "Activity recreada con proceso vivo" dejaba la UI sin track mientras sonaba
  (primer estado del plugin se trata como `first` en `handleState`, sin evento).
- Decisión del usuario: 1.2 y 1.3 se hacen juntos.
- Gotcha build: `pnpm build:android` compila las 4 ABIs (aarch64, armv7, i686, x86_64) y cada
  "Compiling sway" en release tarda varios minutos sin output (parece trabado, no lo está).
  Para probar en el celu (S24 Ultra, arm64) alcanza con
  `cd app && pnpm tauri android build --apk --target aarch64`. Además, si hay otro cargo
  (otro build, rust-analyzer) usando `target/`, queda en "Blocking waiting for file lock".
- Instalado en el S24 (SM-S926B) con `adb install -r` sobre Tailscale: `adb connect
  100.109.147.99:5555` (IP Tailscale del celu "s24"; en LAN era 192.168.0.6:5555). APK firmado
  con la clave Locardium (`CN=Locos`), así que `-r` actualiza sin borrar datos.

## 2026-09-11 · 1.1 Freeze al subir el gain (🧪 falta device)

- `crates/native-audio/android/.../NativeAudioPlugin.kt`:
  - Nuevo `boostHandler` (HandlerThread "native-audio-boost"). `applyBoostLocked` ya no toca el
    `LoudnessEnhancer`: anota `boostWantedSession/boostWantedMb` en el `Deck` y encola
    `reconcileBoost` (coalescing con `boostQueued`). `reconcileBoost` es la vieja lógica, corriendo
    en ese thread. `dispose` → `retireBoostLocked` en vez de `releaseBoost()` directo.
  - `NativeAudioRuntime.setSourceGain` devuelve el snapshot; el `@Command setSourceGain` resuelve
    con él (antes tomaba el lock otra vez con `getState`).
- `app/src/App.tsx` `onGain`: estado local `gainLive` para el knob (se pasa a `PlayerBar` como
  `track` con `gainDb` pisado), escrituras serializadas (`gainInFlight` + `flushGain`), y parche
  de `library`/`plTracks` sólo tras 400 ms sin mover el knob.
- Sin commit (el usuario no lo pidió).
- Verificado: `pnpm tsc --noEmit` OK; Kotlin compila OK con
  `gradlew.bat :tauri-plugin-native-audio:compileDebugKotlin` (desde `app/src-tauri/gen/android`).
  Gotcha: el `JAVA_HOME` del sistema es un JRE 8 sin javac y gradle falla con "does not provide
  the required capabilities: [JAVA_COMPILER]". Para compilar a mano:
  `$env:JAVA_HOME = "C:\Program Files\Android\Android Studio\jbr"` antes del gradlew.
  Falta probar en device: mover el knob de punta a punta rápido y pausar/cambiar playlist a la vez.
- Hallazgos: el plan decía que el enhancer se recreaba al cruzar duck=1.0 — falso, sólo se
  deshabilitaba (plan corregido). La segunda causa real era JS: re-render de toda la TrackTable
  por pixel del knob.
- Usuario pidió agregar "ignorar optimización de batería" en Settings → quedó dentro de 1.3 en
  el plan.

## 2026-09-11 · Planificación Beta 1

- Se armó el plan de bugfix a partir de `notes.txt` (16 bugs + 5 pedidos).
- Investigación completa del código hecha para cada ítem; resultado en `PLAN-BETA1.md`
  (causas raíz con archivo:línea). Nada de código tocado todavía.
- Decisiones del usuario: estabilidad primero; rediseño de UI = fase aparte al final;
  arrastrar tracks hacia afuera de Sway = sí, con `tauri-plugin-drag`.
- Hallazgos clave para no re-investigar:
  - Todo `@Command` del plugin native-audio corre en el UI thread de Android → cualquier cosa
    bloqueante ahí congela el WebView (explica el freeze del gain).
  - `TARGET_LUFS` está en `-6.0` en `crates/core/src/db.rs` pero su comentario dice -14 y
    `app/src/api.ts` exporta -18. No se mide el pico → normalize clipea por construcción.
  - La app nunca manda title/artist/artwork al plugin (`nativeAudio.ts` setSource) → la
    notificación queda vacía con archivos sin tags.
  - No hay foreground service ni wakelock para el sync en Android.
  - `reveal_track` pasa `/select,<ruta>` como un único argumento → explorer no lo parsea.
- Se creó el sistema de continuidad: `CLAUDE.md` (raíz) + `docs/progress/{STATUS,LOG,PLAN-BETA1}.md`.
