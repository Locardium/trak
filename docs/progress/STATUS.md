# Estado actual — Beta 1 bugfix

> Leer esto primero en cada chat. Actualizar al terminar cada tarea.
> Detalle de cada ítem (causa raíz, archivos, fix): `PLAN-BETA1.md`. Historia: `LOG.md`.
> Lista original del usuario: `notes.txt` (raíz, gitignoreado).

**Última actualización:** 2026-09-29
**Fase actual:** ninguna en curso. Fase 4 cerrada y commiteada 2026-09-29 (rama `beta1-fase4`).
**Próximo paso (en este orden):**
1. **Fase 2** (2.1-2.4, estado de conexión con el server — hacerla junta).
2. **Fase 5** (5.1 artefactos del release en `dist/<tag>/` + APK; 5.2 README del server en Linux).
3. Pendiente de confirmación en uso real (hecho, el usuario no lo dijo explícito): 1.2/1.3/1.4
   (lag y notificación de sync con pantalla apagada), 1.6 (botones en background largo), y 1.7
   parte A tras matar el proceso. 1.7 parte B (reanudación Media3 con proceso muerto) en reserva.
4. Fase 6 (rediseño de UI) se planea aparte después.

**Todo lo de las fases 1 y 3 está commiteado** (2026-09-25, rama `beta1-fase1-3`, ver LOG).
Ojo con los finales de línea: varios archivos del repo son CRLF en el índice (`git ls-files
--eol`); si un editor/script los deja en LF, el diff sale entero. Antes de commitear:
`git ls-files --eol | awk '$1=="i/crlf" && $2!="w/crlf"'` debe salir vacío.

**Protocolo de sync cambiado sin compatibilidad** (pedido del usuario): `BlobReq`/`BlobPush`
llevan `index`/`of` obligatorios → PC, celu y server tienen que estar en la misma versión.

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
- 🧪 1.2 Lag de la UI durante el sync (tick 25 ms, sync-progress sin throttle, recarga de librería, sweep R128)
- 🧪 1.3 Sync se corta con la pantalla apagada — FGS dataSync + wakelock + WifiLock hecho 2026-09-25; antes: exención de optimización de batería
  🧪 instalada en el S24 el 2026-09-25 (falta que el usuario la active desde Settings y confirme
  que sigue apareciendo tras un reinicio), falta el foreground service dataSync + wakelock +
  WifiLock (ver plan)
- 🧪 1.4 Transferencia rápida → lenta (hash dentro del lock de DB, library_changed por archivo, timeout 180 s)
- ✅ 1.5 Volver a la app = estado perdido (restore desde plugin/checkpoint + cola y playlist en DB
  + configChanges). Confirmado por el usuario en 14 días de uso real; servicio en foreground con
  pausa sigue afuera (ver plan), no hizo falta hasta ahora.
- 🧪 1.6 Botones de transporte mueren en background. Causa **confirmada** (exit-info): el proceso
  muere con la música pausada y al revivir solo queda el tema del checkpoint. El fallback nativo
  instalado el 2026-09-25 no arregla eso (casi nunca corre); el fix real es 1.7.
- ✅ 1.7 (parte A; B en reserva) Cola de reproducción nativa + reanudación tras muerte del proceso. **Parte A hecha; el
  usuario confirmó que la notificación "funciona perfecta" (2026-09-25)**. Falta probar la
  restauración tras matar el proceso. Detalle parte A: cola en `NativeAudioRuntime` con cues cacheados, persistida
  (archivo en `filesDir` + prefs), next/prev/pase automático nativos, botones → cola sin pasar
  por JS, restore al abrir la app, JS en Android solo empuja la cola y escucha eventos.
  **Parte B pendiente:** reanudación Media3 (`onPlaybackResumption` + `MediaButtonReceiver`) para
  botones con el proceso muerto SIN abrir la app.

## Fase 2 — Sync / servidor
- ⬜ 2.1 Desconexión del server no se muestra
- ⬜ 2.2 Aparece conectado antes de tiempo
- ⬜ 2.3 Pairing viejo no se olvida cuando el token/clave no matchea
- ⬜ 2.4 El celu recibe del server y de la PC a la vez (semáforo global de sync)

## Fase 3 — Audio
- ✅ 3.1 Normalize satura — −14 LUFS + pico medido + techo −1 dBFS al boostear, hecho 2026-09-25
- ✅ 3.2 Notificación Android: título/artista + carátula (thumb cacheado), hecho 2026-09-25
- ✅ 3.3 Stop en la notificación + stop nativo único, hecho 2026-09-25 (botón de la UI sigue chico)

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
- ⬜ 5.1 Todos los artefactos del release en `dist/<tag>/` (+ APK)
- ⬜ 5.2 README: cómo correr el server en Linux

## Fase 6 — Rediseño de UI
- ⬜ Se planea aparte cuando terminen las fases 1-5.

## Quedó a medias / notas abiertas
- 1.6 y la exención de batería (1.3 parcial) ya están instalados en el S24 (2026-09-25, adb sobre
  LAN 192.168.0.6:5555 — Tailscale no respondió esa vez, no hace falta investigarlo). Falta la
  prueba de uso real: activar la exención en Settings, y ver si next/prev por Bluetooth/
  notificación sigue funcionando después de un buen rato en background con la app fuera de
  pantalla.
- Sigue instalada en el celu la app vieja `com.sway.app` (0.1.0) — pendiente que el usuario la
  borre, no urgente.
- Versión (`tauri.conf.json`/`Cargo.toml`) sigue en `1.0.0-1`, nunca se bumpeó.
