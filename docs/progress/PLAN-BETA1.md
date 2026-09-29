# Plan beta 1 — bugfix + mejoras Sway

> Documento de referencia (causas raíz y fixes). El estado de cada ítem vive en `STATUS.md`,
> la historia en `LOG.md`. Los `archivo:línea` son de 2026-09-11: verificar antes de editar.
> Si un fix revela una causa distinta, corregir la sección acá.

## Context

`notes.txt` junta 16 bugs y 5 pedidos de la primera beta pública. Hoy la app es usable pero
tiene tres agujeros que la rompen en uso real: la UI de Android se congela, el sync se muere
con la pantalla apagada y la transferencia arranca rápida y después se traba. El resto son
molestias visuales y de flujo.

Objetivo: cerrar todo en orden de impacto. Decisiones ya tomadas:

- Prioridad: **estabilidad primero**, después visual, después rediseño.
- **Rediseño completo de UI = fase aparte** (se planea por separado). Sí entran acá los dos
  items chicos: título/artista siempre visibles y botón stop en Android.
- Drag hacia afuera de Sway: **sí**, con `tauri-plugin-drag`.

Un hecho estructural explica varios bugs de Android: **todo `@Command` del plugin
`native-audio` corre en el UI thread** (JS `invoke` → `run_on_android_context` → wry `MainPipe`
→ main looper). Lo que bloquee ese thread congela el WebView entero mientras ExoPlayer sigue
sonando en su propio thread.

---

## Fase 1 — Estabilidad Android (bloqueantes)

### 1.6 Botones de transporte (notificacion/lockscreen/Bluetooth del auto) mueren en background
(hallazgo 2026-09-25, reportado por el usuario tras 14 dias usando 1.0+1.1+1.5 en su S24, sin
notes.txt)

Causa: `MainActivity.dispatchMediaButton` solo puede llegar a JS via `live` (la Activity viva).
`NativeAudioService` (el foreground service de audio) sobrevive a que Android destruya la
Activity para liberar memoria -- pasa con la musica sonando igual, sin que nada lo avise -- y
ahi el boton de next/prev quedaba mudo (silent `return`). JS solo pre-encola un tema siguiente
por vez (`setNextTrack(followerOf(id))`), lo que explica el patron exacto reportado: el primer
"next" con la Activity todavia viva funciona (cambia Y pre-encola el proximo), el resto ya no
tienen a donde ir del lado nativo.

Fix (implementado 2026-09-25, no probado en device): `transportInterceptor` devuelve `Boolean`;
si no llega a JS, `transport()` en `NativeAudioPlugin.kt` cae a un fallback nativo -- "next" via
`skipToNext(appContext)` (ya existia, arranca el tema pre-encolado sin JS), "prev" via
`seekTo(0)` (restart, no hay historial de temas sin JS). Limite: sigue habiendo como maximo UN
salto nativo disponible (lo ultimo pre-encolado); sin la Activity viva para re-encolar, un
segundo "next" seguido vuelve a no hacer nada. Arreglo real de fondo requeriria mover
queue/shuffle/repeat fuera de JS.

Sospecha de causa raiz de fondo (no confirmada): Doze / battery-killers de fabricante (Samsung
"sleeping apps") congelando o matando el proceso en background es probablemente lo que fuerza
que Android destruya la Activity en primer lugar. Por eso se adelanto parte de 1.3 (ver abajo):
exencion de "ignorar optimizacion de bateria", pedida por el usuario el 2026-09-11.

**CORRECCION 2026-09-25 (mismo dia, al investigar la cola nativa): el diagnostico de arriba
probablemente esta MAL.** Leyendo tao/tauri-runtime-wry:
- `Rust.create()` (arranca el runtime Rust/Tauri) lo llama `WryLifecycleObserver` registrado en
  `ProcessLifecycleOwner` → corre UNA vez por proceso, no por Activity.
- Pero al destruirse la Activity (sin cambio de config), `tao::ndk_glue::onActivityDestroy` manda
  `WindowEvent::Destroyed`; `tauri-runtime-wry` (lib.rs ~4310) saca la ventana y, si no queda
  ninguna, emite `RunEvent::ExitRequested`; Sway usa `Builder::run(ctx)` sin callback → nadie lo
  previene → `ControlFlow::Exit` → `tao::android::EventLoop::run` → `std::process::exit`.
  **O sea: en Sway, Activity destruida = proceso entero muerto, servicio de audio incluido.** El
  escenario "Activity muerta, servicio sonando" no puede darse (salvo una ventana minima), asi que
  el fallback de `live == null` casi nunca corre.
- Causa alternativa mas probable (NO confirmada): el renderer del WebView es un proceso aparte
  (`com.google.android.webview:sandboxed_process0`). Con la app fuera de pantalla su importancia
  cae (el binding de Chromium depende de la visibilidad), y Android puede congelarlo (cached apps
  freezer, Android 12+) o matarlo aunque el proceso principal tenga foreground service. Con el
  renderer congelado, `dispatchMediaButton` ve `live`/`webView` vivos, postea el
  `evaluateJavascript` y el JS no corre hasta que la app vuelve a pantalla → "tengo que abrir la
  app". El primer "next" funciona porque el freezer tarda en actuar despues de apagar la pantalla.
- Implicancias: (a) la cola nativa (1.7) sigue siendo el fix correcto, con mas razon: cualquier
  diseno donde un boton multimedia necesite que corra JS se rompe con el renderer congelado;
  (b) aparece un problema aparte: deslizar Sway desde Recientes con musica sonando destruye la
  Activity → Tauri hace `process::exit` → se corta la musica (Spotify sigue sonando). Arreglarlo
  requiere prevenir `ExitRequested` en Android y verificar que una Activity nueva re-cree el
  WebView — investigar antes de tocar.
- ~~Pendiente: confirmar con una prueba real en el celu~~ → ver causa confirmada abajo.

**CAUSA CONFIRMADA 2026-09-25 (`adb shell dumpsys activity exit-info com.locardium.sway`):** el
proceso de Sway muere casi todos los dias estando en background con la musica PAUSADA
(`importance=400` = cached en todas las muertes, o sea el servicio ya no estaba en foreground).
Razones en la ultima semana: `OTHER KILLS BY SYSTEM` con `description=AL_Kill : over 1 slots`
(el limitador de apps cacheadas de Samsung, la mas frecuente), `LOW_MEMORY`,
`USER REQUESTED / KILL BACKGROUND` a las 03:39 (auto-optimizacion nocturna de Samsung) y
`REMOVE TASK` (swipe desde Recientes). Secuencia del bug reportado:
1. Musica pausada (a mano, o al desconectarse el Bluetooth del auto) → `NativeAudioService` hace
   `stopForeground(DETACH)` → proceso cacheado, la notificacion queda colgada.
2. Samsung mata el proceso (AL_Kill / low memory / nocturno).
3. Next desde la notificacion/Bluetooth → la app arranca de cero → el restore de 1.5 recupera
   SOLO el tema del checkpoint ("vuelve a la cancion donde se quedo"); la cola en memoria (JS y
   nativa) no existe → "se olvida la cola".
4. Abrir la app → JS recupera la cola persistida en la DB (1.5) → next vuelve a andar.
El renderer congelado y la Activity destruida con el proceso vivo quedan descartados como causa
principal. Ni el fallback de 1.6 ni la exencion de bateria arreglan esto: el proceso muere igual.

**SEGUNDO MECANISMO, CONFIRMADO EN VIVO 2026-09-25 11:49-11:56 (proceso vivo, pid 24638 todo el
tiempo; logcat en scratchpad `repro-notif.log`):** con la app fuera de pantalla un rato, cada
"next" de la notificacion **recarga el mismo tema desde 0** ("reinicia la cancion"):
- 11:51 los next andan (Aprovecha → Baby Boo → Prrrum).
- 11:55:12 Prrrum termina y el player pasa solo (gapless) a Na Na Nau.
- 11:55:16-21, 5 next seguidos: los 5 llegan a Kotlin (`boton multimedia: next` en logcat) y los
  5 recargan **Na Na Nau** desde 0 (BUFFERING → PLAYING pos 0, metadata siempre Na Na Nau).
Explicacion: el JS corre (el `evaluateJavascript` y las promesas del IPC funcionan) pero **React
no re-renderiza con la app en background** (Chromium congela las colas de tareas "freezable" de
una pagina oculta; el scheduler de React usa `MessageChannel`). `playOffsetRef`,
`followerOfRef`, `onAdvancedRef` solo se actualizan en el render, asi que quedan con el
`currentId` viejo (Prrrum, porque el `advanced` hacia Na Na Nau tampoco se renderizo): cada next
calcula "el que sigue a Prrrum" = Na Na Nau y lo recarga. Al abrir la app, React renderiza y todo
se acomoda. Ademas explica el "se olvida la cola": el re-encolado del siguiente tambien usa el
estado viejo.
Por que el pase automatico SI anda en background (observacion del usuario, confirma lo anterior):
el gapless lo hace ExoPlayer solo con el tema pre-encolado, y el re-encolado del siguiente sale de
`onAdvancedRef.current(id)` → `setNextTrack(followerOfRef.current(id))`, que recibe el `id` por
parametro y lee `queueRef` (un ref, siempre al dia) — no depende del `currentId` de React. En
cambio `playOffset` (next/prev) calcula desde el `currentId` de React, congelado en el ultimo tema
que se renderizo ("el tema trabado"): todo lo que avanzo solo se ignora y next vuelve a
"trabado + 1".
**Consecuencia para el diseno:** cualquier logica de reproduccion que viva en React (estado +
refs actualizados en el render) es poco confiable en Android con la app fuera de pantalla. Refuerza
1.7: los botones y las transiciones no pueden depender de JS.

### 1.7 Cola de reproduccion nativa + reanudacion tras muerte del proceso (decision del usuario 2026-09-25: "armar bien la base")

Objetivo: el modelo de Spotify/YT Music. **Kotlin es dueno de la cola; JS la arma y se la
entrega, y en Android pasa a ser espectador.** Y la cola **sobrevive a la muerte del proceso**
(que en el S24 pasa casi a diario, ver causa confirmada de 1.6): cuando llega un boton
(notificacion, Bluetooth del auto, tarjeta multimedia del sistema) con la app muerta, Android
arranca solo el servicio de audio, que recarga la cola guardada y sigue — sin abrir la app ni
depender de JS. Desktop no cambia.

Diseno (revisado 2026-09-25 tras confirmar la causa; nada implementado todavia):
- **Cola en `NativeAudioRuntime`** (plugin) con los **cues ya resueltos** (id, path, gainDb,
  leadMs, audioEndMs, title, artist): el lado nativo nunca necesita preguntarle nada a Rust para
  reproducir. Mas posicion actual, `repeat` ("off"/"track"/"once") y `autoplay`. El shuffle ya
  viene aplicado por JS en el orden. Reusa el staging gapless/crossfade que ya existe.
- **Persistida en disco** por el plugin (archivo JSON en el almacenamiento de la app, escritura
  atomica, en un hilo propio — nunca en el UI thread) al cambiar la cola y al avanzar de tema;
  la posicion dentro del tema ya se guarda en el checkpoint existente.
- **Reanudacion Media3**: `MediaSession.Callback.onPlaybackResumption` devuelve la cola guardada +
  posicion; declarar `androidx.media3.session.MediaButtonReceiver` en el manifest para que un
  boton multimedia con la app muerta arranque el servicio. Verificar que requisitos pide Media3
  1.4.1 / Android 14 para la tarjeta de reanudacion del sistema.
- **Sin puente JNI** (descartado): con los cues cacheados en la cola, el servicio no necesita el
  runtime de Rust — que ademas solo arranca con la Activity (`WryLifecycleObserver` via
  `ProcessLifecycleOwner` desde `WryActivity`), asi que en un proceso levantado solo por el
  servicio Rust ni existe. Se quito `jni` del `Cargo.toml`.
- **Rust**: comando nuevo `queue_cues(ids) -> Vec<TrackCue>` (una sola pasada por la DB, con
  `db_read`) para que JS arme la cola completa sin N round-trips. Extraer la logica de
  `track_cue` a un helper compartido (hoy toma el lock de escritura para solo leer).
- **Comandos nuevos del plugin**: `set_queue` (items + posicion + repeat + autoplay; si el tema
  actual no cambia, no lo corta, solo re-encola el siguiente), `play_queue_index`, `queue_next`,
  `queue_prev`. Los botones multimedia van directo a la cola nativa (sin JS).
- **Invalidacion de cues**: cambios de gain/normalize/crossfade(trim)/borrado de tracks → JS
  re-empuja la cola (o los items afectados). Aceptable que un cue quede viejo unos segundos.
- **JS en Android**: `onPlay`/`playOffset`/shuffle/repeat/autoplay solo empujan a la cola nativa;
  `onTrackEnded`/`setNextTrack`/el hack `NEAR_END_LEAD_MS` dejan de orquestar; `currentId` y el
  modo repeat salen de los estados que empuja el plugin. El restore de 1.5 pasa a leer la cola
  del plugin (fuente de verdad) en vez de reconstruirla por su lado.
- **Pendiente de decidir**: mantener el servicio en foreground unos minutos tras pausar (menos
  muertes por AL_Kill mientras se esta usando) — no reemplaza la reanudacion, la complementa.

### 1.9 Emparejar: el codigo no aparece del otro lado y cerrar el panel congela la UI (reportado 2026-09-25, investigado y arreglado 2026-09-25)

Sintoma: toco "Pair" en un dispositivo → en el OTRO nunca aparece el cartel con el codigo; si en el
que inicio cierro el panel del codigo (la X), se congela la UI de ese lado. Pasa en las dos
direcciones.

**No es de la app dev ni de los datos nuevos**: la logica de pairing no cambio desde el ultimo
commit (salvo `store_device`, que corre despues). Se reprodujo con el MSI instalado del 28/8 en la
PC. La hipotesis 1 del plan (comando sincrono bloqueando el main thread) era FALSA: `connect_peer`
ya corre en su propio hilo; el IPC respondia en ms durante todo el "freeze".

Causas confirmadas (con DevTools remoto en los dos webviews, ver LOG):
1. **El evento se pierde**: `pairing-request` solo se escuchaba en `Sync.tsx`. Si el otro
   dispositivo estaba en Settings/biblioteca, el evento (que se emite una sola vez) no lo veia
   nadie; abrir Sync despues no lo recupera. Reproducido: celu en Settings → nada.
2. **Colision en `Pairing.pending`** (keyed por uid): el pedido no visto de (1) expira a los 120 s
   (`DECISION_TIMEOUT`) y su limpieza hacia `remove(uid)`, borrando el sender del pedido NUEVO con
   el mismo dispositivo → su `recv` da `Disconnected` → rechaza en silencio y el cartel se cierra
   solo. Por eso tras el primer intento fallido tampoco andaba el siguiente.
3. **El "freeze" es un backdrop invisible**: Sync devolvia `<Modal>` del pairing EN EL MISMO LUGAR
   que su `<Modal title="Sync">` → React reusa la instancia. La X llama `close()` → `closing=true`
   → a los 180 ms `decide(false)` vuelve al panel de Sync, pero la instancia reusada queda con
   `closing`: `.modal-backdrop.closing` con opacity 0 y pointer-events auto tapa toda la pantalla
   y se come los toques. Reject / Codes match no pasan por `close()`, por eso solo congela la X
   (o tocar el fondo / Escape).
4. Menor: el receptor no se enteraba de que el iniciador cancelo (no lee el socket mientras espera)
   y seguia mostrando un codigo muerto hasta 120 s.

Fix (2026-09-25):
- `app/src/components/PairingDialog.tsx` (nuevo): listener de `pairing-request`/`pairing-done` +
  el modal del codigo, montado siempre en `App.tsx` (despues de los otros modales, para quedar
  arriba), con `key` por pedido. `Sync.tsx` ya no tiene estado de pairing; su `pairing-done` solo
  limpia `busyPeer` y recarga peers.
- `app/src/styles.css`: `.modal-backdrop.closing { pointer-events: none }` (red de seguridad).
- `app/src-tauri/src/pairing.rs`: `pending` guarda `(id, Sender)`; la limpieza solo borra su propia
  entrada. `ask_user(handle, ev, watch: Option<&Session>) -> Decision` espera en pasos de 250 ms
  y mira el socket: receptor → si el iniciador manda algo (su `Reject`) cierra con ese motivo;
  iniciador → si el receptor contesta primero, "no" corta ya y "si" se guarda y sigue esperando
  la decision local (sin mirar, el peer calla hasta el `PairAck`).
- `crates/core/src/wire.rs`: `Session::peer_has_spoken()` (peek no bloqueante; Ok(0)=cortó,
  Ok(n)=hay mensaje). Test `peer_has_spoken_sees_messages_and_hang_ups_without_consuming`.
- Sin cambio de protocolo: anda contra versiones viejas del otro lado.

### 1.8 Primer vinculo sincroniza todo sin preguntar (reportado por el usuario 2026-09-25)

Sintoma: al vincular un dispositivo por primera vez arranca a bajar toda la biblioteca; destildar
playlists en el medio no lo frena — hay que esperar que termine y borrar a mano, o reiniciar la app.

Causas:
- Sin fila en `device_sync`, el scope de un dispositivo es "all" (`scope::get` default), asi que
  el primer sync planifica la biblioteca entera.
- `engine::sync` arma `plan.pull_files` una vez al empezar y lo recorre entero: un cambio de scope
  durante el loop no se veia.

Fix (implementado 2026-09-25, sin probar en device):
- `app/src-tauri/src/pairing.rs` `store_device` (wrapper de la app; el server usa el de core y NO
  cambia — es archivo, recibe todo): si es el primer vinculo (0 devices y sin fila propia en
  `device_sync`), pone el scope propio en "selected" sin nada marcado y el setting
  `scope_prompt_pending = 1`. "selected + nada" ya lo entiende cualquier version (no hay cambio
  de protocolo): el primer sync trae solo playlists y metadata, justo lo necesario para elegir.
- Comandos `scope_prompt_pending` / `resolve_scope_prompt(everything)` (lib.rs); tarjeta en
  `Sync.tsx` ("What should this device sync?" → Everything / Choose playlists), estilos
  `.scope-prompt` en styles.css.
- `crates/core`: `manifest::wanted_now` (misma regla que `plan`, con las filas de scope leidas en
  el momento + playlists/memberships del remoto) y el loop de pull de `engine::sync` la re-chequea
  entre archivos (solo recalcula si las filas cambiaron; `ScopeEntry`/`DeviceSync` ahora
  `PartialEq`). Test `wanted_now_follows_scope_changes_made_during_the_sync`.
- Limite: si el sync lo inicio el OTRO lado (la PC empujando al celu), su loop de push usa el scope
  del celu del momento del plan; un cambio en el medio no lo frena. Relacionado con 2.4.

Hallazgo en device (2026-09-25, despues del fix de 1.9): tras vincular, el celu quedaba en
"Selected playlists" con **"No playlists yet"** y no se podia elegir nada: NINGUN sync arranca al
crear un vinculo. `autosync::peer_came_online` solo se llama cuando discovery ve al dispositivo por
primera vez (en ese momento todavia no estaba vinculado → se filtra) o en la transicion
offline→online del probe; asi que las playlists llegaban recien con la pasada periodica (10 min) o
con un cambio local. Fix: `pairing.rs connect_inner`, rama `Known::Unknown`, despues del `Hello`
del vinculo nuevo llama `autosync::peer_came_online` (respeta "Sync automatically" y los limites
de bateria/red). Solo el lado que inicio, para no arrancar dos syncs cruzados.

Hallazgo 2 (2026-09-25, usuario): en el selector de playlists del celu, las NO marcadas muestran
**0 temas**. Causa de modelo, no de UI: `list_playlists` cuenta `playlist_tracks JOIN tracks`, y una
fila de `tracks` solo existe si el archivo llego alguna vez. `merge.rs` (seccion Memberships)
descarta la membership si el tema no existe localmente ("the next sync brings it"), y
`manifest::plan` solo sincroniza metadata de temas presentes en los DOS lados. Consecuencia mas
grave que el contador: un dispositivo con seleccion parcial tiene memberships incompletas, y si
sincroniza con un TERCERO (otro celu, server) no puede reenviarle lo que no tiene.
**Resuelto (2026-09-25, el usuario pidio "que muestre cuantos temas tendria cada playlist"):**
tabla `parked_memberships (playlist_uid, track_uid, added_at)`, creada en `db::migrate` (no en
SCHEMA, para que una DB vieja migrada tambien la tenga). `merge::apply` guarda ahi la membership
cuyo tema no existe aca (en vez de descartarla) y la borra cuando la puede linkear; los tombstones
de `track`, `playlist` y `playlist_track` (este con `added_at <= deleted_at`) la limpian.
`db::list_playlists` suma al total las estacionadas que no estan ya linkeadas. El otro lado las
reenvia en cada sync (`changes_for_peer` ve que faltan), asi que el numero se mantiene al dia en
las dos direcciones de sync. Test `merge::tests::parked_memberships_give_the_real_count_until_removed`.
NO hecho (a proposito, no lo pidio): exportarlas en el manifest para que un dispositivo parcial
se las reenvie completas a un tercero. Si aparece ese caso (celu ↔ server sin la PC), es el paso
siguiente. El conteo estacionado no suma a `applied.memberships` (no cambia MetaAck ni tests).

Tambien (mismo reporte): logcat lleno de `symphonia_metadata::id3v2: unsupported frame GEOB/RVAD`
— ruido del analizador R128 al medir cada tema nuevo. El filtro que silencia decodificadores
existia solo en desktop; `android_logger` no tenia filtro. Ahora usa el mismo `LOG_FILTER`.

### 1.0 Loop de reimport del watcher (hallazgo 2026-09-11, no estaba en notes.txt)

Síntoma en device: al abrir la app y cambiar de playlist la UI se cuelga ("explota").
Causa (medida): `perf.log` del celu → `LOCK held 15922 ms by lib.rs:1640`, dos veces seguidas.
`notify` 8.2.0 en inotify vigila también `OPEN`/`CLOSE_NOWRITE` (registry `notify-8.2.0/src/inotify.rs:427`).
Cualquier cosa que abre/lista la carpeta Music genera un evento con path = la carpeta; ese path
no está en `tracks` → pasa el filtro de `spawn_folder_watch` → `import::import_paths` →
`collect_audio` expande el directorio a los 926 archivos → `read_meta` (2 aperturas lofty) +
`hash_file` (blake3 del archivo entero) por cada uno, **con `state.db.lock()` tomado** (~16 s). El
walk vuelve a abrir la carpeta → nuevo evento → loop perpetuo (logcat: 2128 aperturas lofty en 44 s,
~3 GB leídos cada 4 s según `io_stats`).
Fix (hecho): `.filter(|e| !e.path.is_dir())` en `spawn_folder_watch` (`lib.rs`). Pendiente para 1.4:
el import del watcher sigue hasheando con el lock de escritura tomado (viola la regla del CLAUDE.md),
aunque ahora sólo corre para archivos realmente nuevos.

### 1.1 Freeze al subir el gain (notes: "si subo la gain en android se traba la app")

Causa: `Knob` → `App.tsx:754 onGain` (throttle 60 ms) → `set_track_gain` → `track_cue` →
`setSourceGain` (`NativeAudioPlugin.kt:1612`), y ahí, **en el UI thread**, se construye un
`LoudnessEnhancer(sessionId)` (`NativeAudioPlugin.kt:1187`) y se llama `setTargetGain`/`enabled`
(`:1196-1198`). Construir un `AudioEffect` es una llamada binder síncrona a `audioserver`; en
varias ROMs bloquea. Encima el `tickRunnable` de 25 ms y el `fadeRunnable` de 30 ms pelean el
mismo `lock` y también llaman `applyBoostLocked` 33 veces por segundo.

**Corrección (2026-09-11, al implementar):** el enhancer NO se recreaba cuando `duck` cruza 1.0
— sólo se hacía `enabled=false/true`; se recrea únicamente si cambia el session id. Y había una
segunda causa del lado JS: `App.tsx onGain` hacía `setLibrary`+`setPlTracks` **por cada pixel**
del knob → re-render completo de `TrackTable` (sin virtualizar). Además las escrituras
throttleadas no esperaban a la anterior, así que con el UI thread lento se apilaban invokes.

Fix (implementado):
- Todo `LoudnessEnhancer` (crear/soltar/`setTargetGain`/`enabled`) corre en un `HandlerThread`
  (`boostHandler`, "native-audio-boost"). `applyBoostLocked` sólo anota `boostWantedSession`/
  `boostWantedMb` en el `Deck` y encola `reconcileBoost` con coalescing (`boostQueued`
  AtomicBoolean): N cambios mientras audioserver está trabado = 1 llamada más. Deduplica contra
  el último deseo, así el fade (33 Hz) no encola nada salvo cuando el boost cambia de verdad.
  En vez del "debounce 0.5 dB" del plan original se usó este coalescing (el valor final queda exacto).
- `dispose` usa `retireBoostLocked` (el release del efecto también va al thread).
- `setSourceGain` devuelve el snapshot que emitió; el `@Command` resuelve con ése.
- JS: el knob sigue un estado local `gainLive`; las escrituras van de a una (`gainInFlight`);
  las listas se parchean recién cuando el knob queda quieto 400 ms.

### 1.2 Lag de la UI mientras sincroniza (notes: "mientras la app sincroniza en android tiene lag")

Cuatro fuentes acumuladas:
- `FOREGROUND_PROGRESS_TICK_MS = 25` (`NativeAudioPlugin.kt:49`): un round trip Kotlin→Rust→JS
  40 veces por segundo en el UI thread, más `ActivityManager.getMyMemoryState` y
  `PowerManager.isInteractive` por tick (`:1268`), más `SharedPreferences.apply()` (`:1300`).
  → subir el tick a ~250 ms cuando la pantalla está apagada / la app en background, y a ~100 ms
  en foreground; sacar las consultas de sistema del camino del tick (cachearlas).
- `sync-progress` sin throttle: `app/src-tauri/src/lib.rs:281 AppHost::progress` emite por cada
  chunk de 1 MiB. → throttlear igual que `emit_library_changed` (`pairing.rs:36-49`,
  `LIBRARY_EVENT_MIN_MS`), ~200 ms o por porcentaje.
- `library-changed` → `App.tsx:337` recarga la librería entera y re-renderiza `TrackTable`, que
  **no está virtualizada** (`TrackTable.tsx:360` mapea todas las filas). → durante un sync activo,
  acumular y refrescar al final (o refrescar sólo contadores).
- `kick_loudness` arranca el sweep EBU R128 en medio de la transferencia
  (`pairing.rs:47` → `lib.rs:327` → `loudness.rs:352`), con `std::thread::spawn` y **sin bajar
  prioridad** pese al comentario de `loudness.rs:13`. → no arrancar el sweep mientras hay un sync
  en vuelo (chequear `Pairing.active`), y bajar la prioridad del thread de verdad.

**Hecho 2026-09-25 (sin probar en device):** tick de primer plano 25 → 100 ms
(`NativeAudioPlugin.kt`), `sync-progress` throttleado (`sync_notice.rs`), el barrido R128 espera
mientras `sync_notice::busy` (`loudness.rs`, dentro del loop; retoma solo). `library-changed` ya
estaba throttleado a 1,5 s. No hecho: bajar prioridad real del hilo del barrido (no hay `libc` en
deps) ni virtualizar `TrackTable`.

### 1.3 Sync se corta con la pantalla apagada (notes: "si está sincronizando el celu se apaga y deja de sincronizar")

Hoy **no hay nada**: `app/src-tauri/gen/android/app/src/main/AndroidManifest.xml` declara sólo
`INTERNET`, `ACCESS_WIFI_STATE`, `CHANGE_WIFI_MULTICAST_STATE`, `ACCESS_NETWORK_STATE`. El único
wakelock del repo es el del plugin de audio (`crates/native-audio/android/.../AndroidManifest.xml`,
`foregroundServiceType="mediaPlayback"`), que no cubre un sync sin música. `power.rs` es sólo
política de batería (`hold()`, `:200-210`), no mantiene el CPU despierto. Doze mata el socket
(está documentado en `watch.rs:28-36`).

Fix:
- Agregar al manifest de la app: `WAKE_LOCK`, `FOREGROUND_SERVICE`,
  `FOREGROUND_SERVICE_DATA_SYNC`, y un `<service android:foregroundServiceType="dataSync">`.
- Servicio Kotlin nuevo (`SyncForegroundService`) que se arranca cuando empieza un sync y se para
  al terminar, con notificación de progreso. Tomar además un `WifiManager.WifiLock`
  (`WIFI_MODE_FULL_HIGH_PERF`) junto al `MulticastLock` que ya existe en `MainActivity.kt:64-75`.
- Puentearlo desde Rust: arrancar/parar el servicio en `pairing.rs` alrededor de
  `run_sync_blocking` (`:848-911`) y en `watch.rs` mientras hay un `Live`.
- **Exención de optimización de batería (pedido del usuario 2026-09-11) — ✅ HECHO 2026-09-25,
  no probado en device todavía:** Settings (sólo Android) muestra si Sway está exento
  (`PowerManager.isIgnoringBatteryOptimizations`, comando `is_ignoring_battery_optimizations` del
  plugin `native-audio`) y, si no, un botón que abre `ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS`
  (comando `request_ignore_battery_optimizations`; permiso `REQUEST_IGNORE_BATTERY_OPTIMIZATIONS`
  agregado al manifest; Play lo restringe, pero el APK se distribuye por GitHub). Complementa el
  foreground service de sync (todavía sin hacer, ver arriba), no lo reemplaza: sin exención Doze
  igual corta red en ventanas de mantenimiento; con exención pero sin FGS el proceso igual puede
  morir. Se adelantó porque probablemente explica también el bug nuevo de 1.6 (botones de
  transporte muertos en background). OEMs (Xiaomi/Samsung/Huawei) tienen killers propios además
  del de AOSP — falta agregar texto "if controls/sync still stop, see dontkillmyapp.com". No
  arregla 1.1/1.2/1.4 por sí sola (son UI thread / locks, no batería), y el resto de 1.3
  (foreground service + WifiLock para el sync) sigue pendiente.

**Hecho 2026-09-25 (sin probar en device):**
- `crates/native-audio/android/.../SyncService.kt` (nuevo): FGS `dataSync`, canal "Sync"
  (IMPORTANCE_LOW, silencioso), notificación con progreso ("Receiving from PC" / "3 of 128 ·
  archivo", barra indeterminada si no se sabe el total), PARTIAL_WAKE_LOCK (tope 1 h) +
  WifiLock FULL_HIGH_PERF. `START_NOT_STICKY`. Si Android 12+ no deja arrancarlo desde background
  (sin exención de batería), se loguea y el sync sigue sin protección. No overridea
  `onTimeout(Int,Int)` (Android 15, límite 6 h/día) porque el plugin compila contra SDK 34.
- Manifest del plugin: `FOREGROUND_SERVICE_DATA_SYNC`, `ACCESS_WIFI_STATE` y el `<service>`.
- Comandos Kotlin `showSyncNotice`/`hideSyncNotice` llamados DESDE RUST (no desde JS):
  `crates/native-audio/src/lib.rs` guarda el `PluginHandle` en el state y expone
  `show_sync_notice` / `hide_sync_notice` (`run_mobile_plugin`, bloquea un round trip al UI thread
  → sólo desde hilos de sync, throttleado a 1/s). No hace falta declararlos en `build.rs`.
- `app/src-tauri/src/sync_notice.rs` (nuevo): junta el progreso de todos los syncs en curso (por
  peer), throttlea `sync-progress` (200 ms, siempre en cambio de archivo/fin) y maneja la
  notificación; `end(uid)` al terminar cada run (en `run_sync_blocking` y en `serve()` después de
  `serve_requests`) emite `sync-idle` y apaga la notificación con el último.
- Hallazgo: el lado que SIRVE un sync (p.ej. la PC empujándole al celu) no emitía progreso
  (`serve_requests` pasaba `|_, _| {}`). Ahora `engine::serve_requests` emite `Progress` en
  `BlobPush` (recibir) y `BlobReq` (enviar) con `total_files = 0` (= desconocido).
- No hecho: WifiLock/servicio para las sesiones `watch.rs` (`Live`) — sólo transferencias.
- Pedido del usuario (mismo día): el barrido de loudness (archivos nuevos o "Rescan", que es la
  MISMA función: `rescan_analysis` limpia y llama `kick_loudness`) también tiene notificación.
  Comparte servicio y notificación con el sync (`sync_notice::analysis` / `analysis_end`,
  llamados desde `loudness.rs`); el sync tiene prioridad y al terminar el último sync la
  notificación vuelve al análisis. Ícono por tipo (`icon`: upload/download/analyze). Usa el tipo
  `dataSync` aunque es procesamiento local (el tipo `mediaProcessing` es de SDK 35; el plugin
  compila contra 34).

### 1.4 Transferencia arranca rápida y se traba (notes: "manda la música rápido y después lento o se traba")

Se degrada a medida que llegan archivos, no por red:
- `engine.rs:291 host.library_changed(false)` por archivo recibido dispara el recargo completo de
  la UI + `kick_loudness` (ver 1.2).
- `transfer.rs:167` hashea el archivo entero con blake3 y `insert_received` (`:262`) lo vuelve a
  hashear en caso de colisión de uid — **ambos adentro de `host.with_db(...)`** (`engine.rs:535-551`),
  o sea reteniendo el lock de escritura de SQLite durante I/O, justo lo que advierte el doc de
  `engine.rs:44-49`. → hashear fuera del lock; el hash ya se calcula al recibir, reusarlo.
- `engine.rs:629` reconstruye el manifest completo después del loop de archivos.
- Un track trabado tarda `IO_TIMEOUT = 180 s` (`core/pairing.rs:31`) en abortar. → bajarlo para el
  camino de transferencia y agregar detección de "0 bytes en N segundos".

Con 1.2 + estos tres puntos, el "arranca rápido y después lento" debería desaparecer.

**Revisado 2026-09-25 contra el código:** el hash de lo recibido ya se calcula al recibir, FUERA
del lock (`transfer::receive_file`); `insert_received` sólo rehashea en la colisión rara de uid.
`library_changed` por archivo ya está throttleado. Lo único que queda del plan es el timeout de
180 s ante un archivo trabado; se deja así (bajar el timeout arriesga cortar sesiones legítimas).
El resto del "arranca rápido y después lento" lo cubren 1.2 (barrido R128 compitiendo) y 1.3.

### 1.5 Volver a la app = como si se hubiera cerrado (notes: "cuando entro de nuevo a la app a veces es como si se hubiera cerrado")

No se persiste nada de la UI. En recreación de Activity o muerte de proceso, React remonta con
defaults. Se pierden `currentId`, `paused`, `posMs`, `selection`, `plTracks`, `queueRef`
(la cola entera) — `App.tsx:78-150`. El lado nativo sobrevive bien (`activeInstance` se re-apunta
en `MainActivity.installTransportInterceptor`, `:196`).

Fix:
- Persistir `currentId` / `posMs` / `selection` / cola con el `useUiSetting` que ya existe
  (`app/src/uiSettings.ts`) — guarda en la DB, no en memoria.
- Al montar, rehidratar desde ahí **y** desde el checkpoint nativo que ya existe y nadie usa:
  `getProgressCheckpoint` / `clearProgressCheckpoint` (`nativeAudioPlugin.ts:83-85`, runtime
  `NativeAudioPlugin.kt:859/875`) — cero llamadores hoy en `app/src`.
- Agregar `density|fontScale|navigation|touchscreen|layoutDirection` a `configChanges` en el
  manifest (`AndroidManifest.xml:25`) para no recrear la Activity por cambios de tamaño de fuente.
- Mantener el proceso vivo mientras hay música pausada: hoy `NativeAudioService.kt:92-103` hace
  `stopForeground(DETACH)` en cuanto la notificación deja de ser *ongoing*.

**Implementado 2026-09-11 (versión release):**
- Causa extra encontrada: con el proceso vivo y la Activity recreada, el nativo sigue sonando
  pero `nativeAudio.ts handleState` trata el primer estado como `first` y no emite nada → React
  quedaba con `currentId = null` mientras sonaba música.
- `nativeAudio.ts restorePlayback()`: (1) `getState` — si el plugin tiene `trackId`, se adopta;
  (2) si no, `getProgressCheckpoint` → `setSource` + `seekTo` **pausado** (no llama `play`, así
  que no arranca el servicio ni la notificación). `restoreCancelled` evita pisar un track que el
  usuario eligió mientras tanto. `stopPlayback` borra el checkpoint (si no, un Stop resucita).
- `App.tsx`: effect de restore al montar (sólo Android, vía `api.ts restorePlayback`); cola
  persistida en `ui_setting` `queue` (JSON) vía `setQueue`; `selection` pasó a `useUiSetting`
  (`'library'` o id de playlist), sólo Android.
- Manifest: `configChanges` + `density|fontScale|navigation|touchscreen|layoutDirection`.
- **NO hecho (queda para después):** mantener el servicio en foreground con música pausada
  (notificación no descartable + riesgo de `ForegroundServiceStartNotAllowedException`); con el
  restore desde checkpoint, que muera el proceso ya no pierde el estado. Tampoco se valida que la
  playlist restaurada siga existiendo (si la borró un sync, se ve la lista vacía hasta tocar
  Library).

---

## Fase 2 — Sync / servidor: estado correcto

### 2.1 No se muestra la desconexión del server (notes: bug 1)

`discovery.rs:471 probe_once` **saltea** cualquier uid con watcher vivo (`watchers.is_live`), y
`Drop for Live` (`watch.rs:513-526`) a propósito no pone `online = false`. Con `RETRY_AFTER_DROP = 3 s`
(`watch.rs:72`) el watcher se reconecta antes de que corra el probe de 10 s, así que la fila puede
quedar verde para siempre. Además el server no tiene registro mDNS, así que `ServiceRemoved`
(`discovery.rs:393`) nunca lo puede apagar.

Fix: `Drop for Live` marca `online = false` (con un pequeño grace de reconexión); dejar de saltear
el probe para peers con watcher, o al menos probar cuando el watcher no está `Live`; bajar
`READ_TIMEOUT` (`watch.rs:52`, hoy 120 s) a ~2 heartbeats reales; en `run_sync_blocking`
(`pairing.rs:848-911`) y `connect_peer` (`:369-388`) marcar `mark_unreachable` también cuando el
error no es un `std::io::Error` (hoy sólo ahí, `:379`).

### 2.2 Aparece conectado antes de tiempo (notes: bug 2)

Un peer resuelto por mDNS **nace `online: true`** (`discovery.rs:366-378`) antes de cualquier probe
TCP; `add_manual` (`:203-222`) sí nace en `false`. `restore_manual_peers` corre en `lib.rs:1799`,
**antes** de `spawn_prober` (`:1803`), así que no hay probe en t=0. El frontend renderiza el badge
directo de `p.online` (`Sync.tsx:604-616, 879-889`) y no tiene un estado "verificando".

Fix: nacer en `false` siempre y dejar que el probe confirme; probar una vez al arrancar antes de
mostrar; agregar un tercer estado `checking` al `Peer` y renderizarlo distinto en `Sync.tsx`.

### 2.3 Pairing viejo no se olvida cuando el código no matchea (notes: bug 3)

Dos caminos, ninguno borra la fila de `devices`:
- Token inválido: `crates/server/src/serve.rs:178-183` → `pairing.rs:557-559`. Nadie llama a
  `forget_device`. Ojo: el token sólo se chequea en `Msg::PairRequest`; un device ya emparejado
  manda `Msg::Hello` (`serve.rs:73`), así que **rotar el token del server no invalida los pairings
  existentes** — el pairing es por clave, no por token.
- Key mismatch: `core/pairing.rs:72-83 known_state` → `warn_key_mismatch` (`pairing.rs:1014-1031`)
  que sólo escribe un `sync_log`. El comentario en `core/pairing.rs:194-196` dice que es a propósito.

Consecuencia: `watch.rs:553-560 still_paired` sigue devolviendo true y el watcher loopea contra un
device que jamás va a autenticar, y `restore_manual_peers` lo resucita en cada arranque.

Fix: en token rechazado o key mismatch persistente, marcar el device como `invalid` y ofrecer un
"olvidar" de un click en `Sync.tsx`; frenar watcher y autosync para ese uid; reusar la lógica de
`unpair` (`pairing.rs:1046-1077`) en vez de escribir una nueva.

### 2.4 El celu recibe del server y de la PC a la vez (notes: bug 6)

`Pairing.active` es un `HashSet` **por uid** (`pairing.rs:65-98`), o sea sólo evita dos corridas
contra el mismo peer. El orden "LAN antes que server" (`autosync.rs:316-366` + `wait_until_idle`,
`pairing.rs:817-837`) sólo restringe lo que este device inicia; el push de la PC entra por
`engine::serve_requests` (`engine.rs:236-292`), fuera de esa vista. Y `watch.rs:442-446 catch_up`
llama `lan_peers` en un instante en que la PC todavía no fue probada online, así que arranca el
sync con el server igual.

Fix: convertir el guard en un **semáforo global de sync entrante/saliente** (1 sola sesión de
transferencia por vez en el device), incluyendo el lado pasivo en `serve_requests`; que el lado
pasivo pueda rechazar con "ocupado" y que el otro reintente; que `catch_up` espere una ventana de
descubrimiento antes de decidir que no hay peers LAN.

---

## Fase 3 — Audio: normalización y Android playback

### 3.1 Normalize volume satura (notes: bug 8)

`crates/core/src/db.rs:760` tiene `TARGET_LUFS = -6.0` mientras su propio comentario dice `-14`, y
`app/src/api.ts:225` exporta `-18`. Tres números para lo mismo. Con target −6, un master normal de
−14 LUFS pide **+8 dB** y uno de −18 pide los **+12 dB** de `MAX_GAIN_DB` — sobre archivos que ya
pican cerca de 0 dBFS, o sea clipping por construcción. Y **nunca se mide el pico**:
`loudness.rs:69 measure()` calcula sólo `ebur128::Mode::I`; `Analysis` (`:48`) no tiene campo de peak.

Fix:
1. Unificar el target en un solo lugar y exponerlo a JS desde Rust (borrar la constante duplicada
   de `api.ts:225`). Valor: −14 LUFS (lo que ya dice el comentario).
2. Medir true peak en la misma pasada (`ebur128` `Mode::TRUE_PEAK`), guardar `peak_dbtp` en
   `tracks` (migración en `db.rs:352`, donde ya está el patrón de columnas agregadas).
3. `playback_gain_db` (`db.rs:865`) topea la ganancia para que `peak + gain <= -1 dBTP`. Sin peak
   medido, no boostear más de +3 dB.
4. Reaplicar el gain al track que ya está sonando cuando se togglea normalize: hoy
   `api.ts:189 applyPlaybackPrefs` sólo manda crossfade y output device.

**Hecho 2026-09-25 (sin probar en device):** el −6 NO fue decisión: entró así en el commit
original 894dca3 mientras el comentario y los tests del mismo commit decían −14 (eran 3 de las 4
fallas "preexistentes"). `TARGET_LUFS = -14`; constante duplicada de `api.ts` borrada (no se
usaba). Pico: **sample peak** (`ebur128::Mode::SAMPLE_PEAK`, casi gratis) y no true peak (4x de
sobremuestreo, minutos más por biblioteca en el celu); columna `tracks.peak_db` (migración +
`NEEDS_ANALYSIS` → toda la biblioteca se re-mide sola una vez, con la notificación de análisis).
`playback_gain_db(gain, lufs, peak, normalize)`: con normalize, un BOOST no pasa
`PEAK_CEILING_DB = -1` (margen por medir muestras); los cortes nunca se limitan; sin pico medido
el boost topea en `UNKNOWN_PEAK_MAX_BOOST_DB = 3`; con normalize apagado el trim manual queda
libre. Toggle de normalize: desktop ya reaplicaba (`set_playback_prefs`); Android re-empuja la
cola pero `NativeAudioRuntime.setQueue` NO reaplicaba el gain al tema actual → ahora sí.
Test nuevo `a_boost_stops_at_the_peak_ceiling`; también se arregló el test de engine
(`editing_both_sides...` comparaba una lista ordenada contra una sin ordenar) → **suite de core
130/130 en verde**.

### 3.2 La notificación no muestra nada (notes: bug 12)

**La app nunca manda metadata.** `nativeAudio.ts:121 setSource` y `:155 setNextSource` omiten
`title`, `artist` y `artworkUrl`, aunque `NativeAudioSource` (`nativeAudioPlugin.ts:43-59`) y
`SetSourceArgs` (`NativeAudioPlugin.kt:125-139`) los aceptan. Entonces `buildMediaItem`
(`NativeAudioPlugin.kt:1329`) arma un `MediaMetadata` vacío y lo único que se ve es lo que ExoPlayer
saque del contenedor — nada en un WAV sin tags. La carátula tampoco: `getCurrentLargeIcon`
(`NativeAudioService.kt:78`) devuelve **el ícono del launcher**, siempre.

Fix: pasar `title`/`artist`/`artworkUrl` desde `nativeAudio.ts` (los datos ya están en el `Track`);
escribir la miniatura a un archivo cacheado y pasar su URI (hoy `cover.rs:27 thumb_data_url` sólo
produce un data URL para el WebView); setear `artworkUri` en `buildMediaItem`. En Android 13+ la
shade la dibuja el sistema desde el `MediaSession`, así que el arreglo tiene que ir en
`buildMediaItem`/`setSource`, no sólo en el adapter de la notificación.

**Hecho 2026-09-25 (sin probar en device):** title/artist ya iban (1.7). Carátula: `TrackCue`
lleva `artwork` = ruta del thumb cacheado (`cover::Covers::file`, 4.2); `track_cue` lo genera si
falta, `queue_cues` sólo usa los existentes (no decodificar cientos). Buscado DESPUÉS de soltar el
lock (hash leído en la query, campo `#[serde(skip)] hash`). `artworkUrl` → `artwork` (ruta) en
Kotlin/TS y en la cola persistida; `buildMediaItem` usa `fileUri()`. `NativeAudioService
.getCurrentLargeIcon` decodifica el thumb (cacheado por ruta) y sólo cae al ícono de la app sin
carátula. JS: `toQueueItem` es el único armado de fuente (set/next/cola).

### 3.3 Botón stop en Android (notes: AGREGAR)

Hoy el stop existe sólo como un badge de 20×20 px sobre la carátula (`PlayerBar.tsx:133`,
`styles.css:484-505` bajo `@media (hover: none)`), y la notificación lo tiene desactivado
(`NativeAudioService.kt:118 setUseStopAction(false)`).

Fix: `setUseStopAction(true)`, `ACTION_STOP` en el `IntentFilter` de `MainActivity.kt:209`, rama
`"stop"` en `transportInterceptor` (`NativeAudioPlugin.kt:372/1133`) y en `__swayMediaButton`
(`App.tsx:293`), y que `stopPlayback` (`nativeAudio.ts:187`) además pare el servicio — hoy pausa y
hace seek a 0 pero deja la notificación colgada.

**Hecho 2026-09-25 (sin probar en device):** el nativo es dueño del stop:
`NativeAudioRuntime.stop()` (vacía cola memoria+disco, next, para y descarga los decks, borra
checkpoint, para el servicio → se va la notificación, emite estado sin track). Comando `stop`
(build.rs + permissions), `override fun stop()` en `wrapForSession` y `setUseStopAction(true)`.
JS: `stopPlayback` = `nativeStop()` (reemplaza la secuencia setQueue([])/setNextSource(null)/
pause/seek/clearCheckpoint); `handleState` emite `stopped` si el player queda sin track sin que la
app lo pidiera → `App.tsx` limpia la UI. No hecho: agrandar el botón de stop de la UI (badge de
20 px sobre la carátula en táctil).

Otro hallazgo de la misma tanda (usuario): "Session Guaratech #1 Kid Wazi" no sonaba en el celu.
Causa: JS mandaba `'file://' + ruta` sin codificar y Kotlin `setUri(String)` → `Uri.parse` corta
en `#` (fragmento). Fix: JS manda la ruta; Kotlin `fileUri()` = `Uri.fromFile(File(...))` (quita
un `file://` viejo de colas guardadas). Afectaba cualquier nombre con `#`, `?` o `%`.

---

## Fase 4 — Bugs visuales y flujo en desktop

### 4.1 La música cargada aparece en la otra playlist (notes: bug 5)

Race clásico. `App.tsx:160-163 refreshPlaylistTracks` y el effect de selección (`:195-201`) hacen
`await` y después `setPlTracks` sin revalidar la selección. El disparador concreto es el import por
drop: `osDropRef.current` (`:700-843`) captura el binding viejo y en `:834` hace
`refreshPlaylistTracks()` cuando el usuario ya cambió de playlist. Se repara solo al volver, que es
exactamente el síntoma reportado.

Fix: guardar el estado como `{ playlistId, tracks }` y descartar cualquier respuesta cuyo
`playlistId` no sea el actual, más un contador de request en `refreshPlaylistTracks`. Aplicar el
mismo guard en `App.tsx:341, 986, 993, 1001, 1010, 1298, 1356`. Ya existe el patrón `alive` en
`Cover.tsx:56` para copiar.

### 4.2 Las carátulas tardan al scrollear rápido (notes: bug 13)

`Cover.tsx` usa un `IntersectionObserver` por fila **sin rootMargin**, así que no pide nada hasta
que la fila ya está en pantalla, no cancela nada y dispara cientos de `invoke('cover_thumb')` de
golpe. Cada miss en Rust (`lib.rs:437-449` → `cover.rs:29-41`) es: parse completo de tags con lofty
+ decode de imagen + resize a 256 + reencode JPEG + base64 por el bridge IPC. El caché
(`state.covers`, `lib.rs:170`) es sólo en memoria y arranca frío en cada sesión. Y la tabla no está
virtualizada (`TrackTable.tsx:360`).

Fix, en orden de rendimiento por esfuerzo:
1. `rootMargin` grande en el observer + cola con concurrencia limitada (~4) y cancelación de filas
   que ya salieron de pantalla.
2. Caché de miniaturas **en disco** (una por hash de track), para que el segundo arranque sea instantáneo.
3. Virtualizar `TrackTable` — se puede diferir a la Fase 6, pero ayuda acá también.

**Hecho 2026-09-25 (pedido del usuario: "que se rendericen todas las carátulas al llegar"),
sin probar:** `app/src-tauri/src/cover.rs` ahora tiene `Covers`: caché en disco por content hash
(`<hash>.jpg`, o `<hash>.none` vacío si el archivo no tiene carátula; escritura tmp+rename), más
la memoria por id de antes. `Covers::warm` genera en background las que faltan (espera mientras
`sync_notice::busy`). `kick_loudness` pasó a `kick_background` y lanza análisis + carátulas (mismos
8 lugares: arranque, import, fin de sync, rescan...). `cover_thumb` lee path+hash con `db_read` y
decodifica fuera del lock. Carpeta: desktop `<Music>/Sway/covers` (junto a la DB, no en
`library`); Android `app_cache_dir()/covers` (ahí la carpeta de datos ES la de música que mira el
watcher). No hecho: rootMargin/cola en `Cover.tsx`, y borrar thumbs de hashes que ya no existen.

**Hecho 2026-09-25 (sin probar):** cola + rootMargin en `Cover.tsx`. Ojo: el observer necesita
`root = .table-wrap` (el contenedor que scrollea); con el viewport como root el overflow de la
tabla recorta las filas y el `rootMargin` no tiene efecto. Sigue sin hacer: limpiar thumbs viejos.

### 4.3 "Reveal in Explorer" no lleva al tema (notes: bug 14)

`lib.rs:507-531 reveal_track` pasa `"/select,C:\ruta"` como **un solo argumento**; el quoting de
Rust lo envuelve en comillas y explorer.exe no lo parsea, así que abre una ventana genérica.
(Precisión 2026-09-25: Rust sólo pone comillas si la ruta tiene espacios/tabs, así que sin
espacios andaba. Fix hecho con `raw_arg`, ver LOG.)

Fix: pasar dos args (`.args(["/select,", &path])`) o armar la command line cruda. De paso, en Linux
usar `dbus-send org.freedesktop.FileManager1.ShowItems` en vez de abrir sólo la carpeta padre
(`:523-530`).

### 4.4 Drag & drop (notes: bug 15)

**Hacia adentro** cae siempre al final: el handler de `onDragDropEvent` (`App.tsx:844-867`) sólo
busca un nodo del sidebar y nunca calcula un índice de inserción; `osDropRef` (`:702-843`) hace
`addTracksToPlaylist` que appendea (`:829`). La lógica de posición ya existe y no está cableada:
`dnd.ts:42-71 resolveTarget` devuelve `{type:'insert', index}`.

**Hacia afuera** no existe: cero `startDrag`, cero `dataTransfer` en `app/src`, y con
`dragDropEnabled` en true (default de Tauri) el webview no puede hacerlo solo.

Fix:
- Cablear `resolveTarget` en el camino del drop del OS y llamar `reorderPlaylistTracks` con el
  índice calculado, igual que hace `onReorder` (`App.tsx:991-994`). Mostrar el indicador
  `dropInsertIndex` también durante el drag del OS. Verificar el escalado por
  `devicePixelRatio` (`App.tsx:849`) en pantallas que no están al 100 %.
- Agregar `tauri-plugin-drag` a `app/src-tauri/Cargo.toml` y `app/package.json`, y llamar
  `startDrag` con las rutas de los tracks seleccionados desde el `onMouseDown` de la fila.

**Hecho 2026-09-25 (sin probar), distinto al plan:** en vez del plugin se usa su crate de abajo
(`drag`) desde un comando propio `drag_tracks_out(ids)` (rutas resueltas en Rust, sin paquete JS
ni capability). No arranca en `onMouseDown`: el drag interno sigue igual y cuando el mouse sale
de la ventana (`onExit` en `dnd.ts`) se pasa al drag del OS. Detalle en LOG.

### 4.5 Título / artista siempre visibles (notes: AGREGAR)

No se ocultan por lógica sino por CSS: `.np-text strong` y `.np-artist` (`styles.css:506-508`) usan
`white-space:nowrap` + ellipsis, y con `.np{min-width:0}` (`:477`) la columna de texto es la primera
que colapsa cuando la ventana se angosta. El player entero desaparece si no hay `playerTrack`
(`App.tsx:~1227`).

Fix: darle un `min-width` real a la columna de texto en el grid del player (`styles.css:448`),
permitir dos líneas, y — en anchos chicos — reordenar en vez de colapsar. El detalle fino queda para
la Fase 6.

---

## Fase 5 — Release y documentación

### 5.1 Todos los releases en una carpeta (notes: AGREGAR)

Hoy los artefactos quedan desparramados y `scripts/release-win.js` los busca por ruta hardcodeada
(`:99-118`): MSI en `target/release/bundle/msi`, server win en `target/release/`, server linux en
`target/x86_64-unknown-linux-musl/release/`, y el APK en
`app/src-tauri/gen/android/app/build/outputs/` **que ningún script recoge**.

Fix: paso de staging en `release-win.js` después del build → copiar todo a `dist/<tag>/` con nombres
finales (`Sway-<tag>.msi`, `sway-<tag>.apk`, `sway-server-windows.exe`, `sway-server-linux`), y que
`gh release create` suba desde ahí. Agregar `pnpm build:android` al release. Agregar `dist/` al
`.gitignore`. De paso: `scripts/bump-version.js` no bumpea `crates/server/Cargo.toml`.

### 5.2 Cómo usar el server, sobre todo en Linux (notes: AGREGAR)

La documentación buena ya existe en `crates/server/README.md` (build con `cargo-zigbuild` a
`x86_64-unknown-linux-musl`, `config.toml` completo, `SWAY_SERVER_TOKEN`, unit de systemd) pero el
README raíz no la linkea y sólo tiene tres líneas de `pnpm` scripts.

Fix: sección "Running the server" en el `README.md` raíz con el quickstart de Linux (bajar el
binario del release, primer arranque que escribe `config.toml` y sale — `config.rs:73-99`, editar,
systemd) y link a `crates/server/README.md`. Agregar un `config.example.toml` al repo (el real está
gitignoreado porque tiene el `pair_token`).

---

## Fase 6 — Rediseño de UI (se planea aparte)

Sólo el inventario, para dimensionar: `styles.css` 1112 líneas / 45 KB, archivo único, tokens
hechos a mano en `:root` (`:1-59`, colores en oklch, escala de espaciado, z-index, easing), sin
theming ni light mode. `App.tsx` 1385 líneas concentra shell + estado + DnD + menús + modales.
`components/`: `Sync.tsx` 1058, `TrackTable.tsx` 476 (sin virtualizar), `Settings.tsx` 385,
`PlayerBar.tsx` 234, `Sidebar.tsx` 217, `Knob.tsx` 168, `Modal.tsx` 155, `Cover.tsx` 89,
`ContextMenu.tsx` 84, `RightPanel.tsx` 52, `Switch.tsx` 25. Iconos de `lucide-react`.

Prerrequisitos que salen de las fases anteriores y conviene tener listos antes: virtualización de
`TrackTable` (4.2), extracción de estado de `App.tsx`, y el layout del player (4.5).

---

## Verification

Por fase, en este orden:

**Fase 1 (Android).** Build con `pnpm build:android` e instalar en el device real.
1. Reproducir un track, mover el knob de gain de punta a punta rápido — la UI tiene que seguir
   respondiendo (pausa, cambio de playlist) durante y después.
2. Arrancar un sync de ~50 tracks y scrollear la lista mientras corre: sin freezes.
3. Con el sync corriendo, apagar la pantalla 5 minutos → al volver tiene que haber seguido
   (verificar contra el conteo del peer y `adb shell dumpsys power | grep -i wake`).
4. Medir el throughput del primer track vs el número 40 del mismo sync: no debería degradarse.
5. Reproducir, salir de la app, abrir 3 apps pesadas, volver → track, posición y playlist intactos.

**Fase 2 (sync).** Con la PC + el celu + el server:
1. Matar el proceso del server → la fila tiene que ponerse gris en menos de ~15 s.
2. Reiniciar la app con el server caído → nace gris, no verde.
3. Cambiar el `pair_token` del server y reconectar → error claro + opción de olvidar el device, y
   sin loop de reintentos infinito en el log.
4. Cargar 20 tracks en la PC y después abrir el celu → una sola fuente por vez en `sync_log`.

**Fase 3 (audio).** `cargo test -p sway-core` (los tests de `playback_gain_db` en `db.rs:1294-1345`
hay que actualizarlos con el nuevo target y el cap de peak). Después, a oído: un track masterizado
hot con normalize on no debe distorsionar. Un WAV sin tags tiene que mostrar título y artista en la
notificación.

**Fase 4 (desktop).** `pnpm dev`:
1. Arrastrar una carpeta grande a una playlist y cambiar de playlist antes de que termine → los
   tracks aparecen en la correcta.
2. Scrollear 1000 filas rápido → las carátulas aparecen sin hueco visible; reabrir la app y repetir
   (caché en disco).
3. Click derecho → Reveal in Explorer sobre una ruta con espacios → el archivo queda seleccionado.
4. Soltar un archivo en el medio de la lista → entra en esa posición. Arrastrar un track de Sway a
   una carpeta del Explorer → copia el archivo.

**Fase 5.** `pnpm release:win 1.0.0-beta.2` en un clon limpio → `dist/v1.0.0-beta.2/` con los cuatro
artefactos y el release de GitHub con los cuatro adjuntos. Seguir el README de Linux de cero en una
VM Ubuntu hasta emparejar un device.
