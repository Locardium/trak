// Playback backend for Android/iOS: the vendored native-audio plugin
// (Media3/ExoPlayer, `crates/native-audio`), validated in Phase 0.5
// (spike-android-audio/). Unlike desktop, this plugin is controlled directly
// from JS — there's no Rust Player to wrap here, the native plugin itself is
// the bridge.
import { invoke } from '@tauri-apps/api/core';
import {
  initialize,
  setSource,
  setNextSource,
  skipToNext,
  play,
  pause,
  seekTo as nativeSeekTo,
  getState,
  getProgressCheckpoint,
  addStateListener,
  setVolume as nativeSetVolume,
  setCrossfade as nativeSetCrossfade,
  setSourceGain,
  listOutputDevices as nativeListOutputDevices,
  setOutputDevice as nativeSetOutputDevice,
  isIgnoringBatteryOptimizations,
  requestIgnoreBatteryOptimizations,
  setQueue as nativeSetQueue,
  setQueueMode as nativeSetQueueMode,
  queueNext,
  queuePrev,
  restoreQueue,
  stop as nativeStop,
  type NativeAudioQueueItem,
  type NativeAudioOutputDevice,
  type NativeAudioState,
} from './nativeAudioPlugin';

export type { NativeAudioOutputDevice };
export { isIgnoringBatteryOptimizations, requestIgnoreBatteryOptimizations };

/// Where the file is and how loud to play it, worked out by the same Rust
/// code the desktop player uses (`track_cue` -> `db::playback_gain_db`). The
/// level has to come from there: it depends on the track's saved trim, its
/// measured loudness and the normalize preference, none of which this side
/// has.
interface TrackCue {
  id: number;
  /// Shown by the notification and the lock screen. Sent explicitly because
  /// the system otherwise falls back to the file's own tags, and a file
  /// without tags (a WAV, typically) showed nothing at all (bug 3.2).
  title: string;
  artist: string;
  path: string;
  /// Cached cover thumb (a file path), `null` if not made yet or none.
  artwork: string | null;
  gainDb: number;
  /// Where the audio really starts and ends. Zero for either edge means the
  /// track has not been measured yet and plays untrimmed.
  leadMs: number;
  audioEndMs: number;
}

const trackCue = (id: number) => invoke<TrackCue>('track_cue', { id });

/// Cues for a whole queue in one trip. Tracks that no longer exist are left
/// out, so callers look entries up by `id` rather than by position.
const queueCues = (ids: number[]) => invoke<TrackCue[]>('queue_cues', { ids });

/// What the plugin needs to play one track, from its cue. One place, so the
/// queue, the current track and the staged next one can't disagree.
function toQueueItem(cue: TrackCue): NativeAudioQueueItem {
  return {
    id: cue.id,
    src: cue.path,
    title: cue.title || undefined,
    artist: cue.artist || undefined,
    artwork: cue.artwork ?? undefined,
    gainDb: cue.gainDb,
    leadMs: cue.leadMs,
    audioEndMs: cue.audioEndMs,
  };
}

let initialized: Promise<unknown> | null = null;
/// Initializes the plugin once, and only *remembers* it once it worked.
///
/// A failure is deliberately not cached. The first call happens while the app
/// is still starting, which is exactly when the plugin is most likely to not
/// be answering yet — and a rejected promise left in `initialized` would be
/// handed to every later caller, so one early miss would take out volume,
/// gain, crossfade and the queue for the rest of the session with nothing to
/// show for it.
function ensureInitialized() {
  if (!initialized) {
    initialized = initialize().catch((e) => {
      initialized = null;
      throw e;
    });
  }
  return initialized;
}

/// What the plugin has staged behind the current track, and how long the
/// overlap is. Both are mirrored here so `playTrack` can tell an ordinary
/// "next" apart from one the plugin can already crossfade into.
let stagedNextId: number | null = null;
let crossfadeSecs = 0;

/// The track the app believes is playing. Compared against the plugin's
/// `trackId` to spot a transition the app didn't ask for — which is exactly
/// what gapless and crossfade produce.
let lastTrackId: number | null = null;

/// A track change the app asked for and the plugin hasn't reported yet.
///
/// The plugin keeps pushing state while `setSource` is in flight, and that
/// state still names the OLD track. Without this, those few ticks look like a
/// transition back to the track we just left — the app would follow them and
/// bounce. States are held back until the id we asked for shows up, or until
/// the deadline says the request went nowhere.
let awaitedId: number | null = null;
let awaitedUntil = 0;
/// How long to keep holding them back. Long enough to cover opening a file off
/// slow internal storage, short enough that a `setSource` that silently failed
/// doesn't freeze the UI.
const AWAIT_TRACK_MS = 3000;

function expect(id: number) {
  awaitedId = id;
  awaitedUntil = Date.now() + AWAIT_TRACK_MS;
}

/// How many `setNextSource` calls haven't come back yet. While any is in
/// flight the plugin's `nextTrackId` is still the previous one, so it isn't
/// trusted over what was just requested.
let stagingInFlight = 0;

/// What the app should show as loaded when it (re)mounts. See `restorePlayback`.
export interface RestoredPlayback {
  id: number;
  posMs: number;
  playing: boolean;
  /// The player came back with a queue of its own (Fase 1.7). When it
  /// didn't, the app hands it the one it kept.
  hasQueue: boolean;
}

/// Set as soon as the user plays or stops something, so a restore that was
/// still on its way can't load the old track over the one just picked.
let restoreCancelled = false;

/// Picks up where the player really is when the WebView starts from zero.
///
/// Two ways to get here, and both used to look like a fresh app:
/// - The Activity (and its WebView) was rebuilt but the process lived on. The
///   plugin is still playing — or holding a paused track — so its state is
///   the answer. Only the React side forgot.
/// - The process died (paused music leaves the service out of foreground, so
///   Android is free to reclaim it). The plugin is empty, but it wrote a
///   checkpoint on every pause: that track is loaded back, paused, at that
///   position, and play resumes it.
export async function restorePlayback(): Promise<RestoredPlayback | null> {
  await ensureInitialized();
  // The native queue first (Fase 1.7): in a fresh process it reads the
  // queue back from disk and loads its current entry, paused where it
  // stopped — after which the plugin's own state is the answer below. The
  // checkpoint path further down only covers a first run without a saved
  // queue.
  await restoreQueue().catch(() => {});
  const st = await getState();
  if (st.trackId != null && st.trackId > 0) {
    lastTrackId = st.trackId;
    return {
      id: st.trackId,
      posMs: Math.round((st.currentTime || 0) * 1000),
      playing: st.isPlaying,
      hasQueue: (st.queueLength ?? 0) > 0,
    };
  }
  const cp = await getProgressCheckpoint().catch(() => null);
  if (!cp || cp.id <= 0 || restoreCancelled) return null;
  // The track may be gone since (deleted, or synced away): then there's
  // nothing to go back to.
  const cue = await trackCue(cp.id).catch(() => null);
  if (!cue || restoreCancelled) return null;
  quiet();
  expect(cp.id);
  stagedNextId = null;
  await setSource({ ...toQueueItem(cue), id: cp.id });
  if (cp.currentTime > 0) await nativeSeekTo(cp.currentTime);
  return { id: cp.id, posMs: Math.round(cp.currentTime * 1000), playing: false, hasQueue: false };
}

// --- Native queue (Fase 1.7) -------------------------------------------------
//
// On Android the player owns the queue. These hand it over and step it; the
// app learns what is playing from the plugin's state events (`advanced`),
// the same way it learns about a gapless hand-off.

/// Hands the whole queue to the player and starts `startId`.
export async function playQueue(
  ids: number[],
  startId: number,
  repeat: string,
  autoplay: boolean,
) {
  restoreCancelled = true;
  await ensureInitialized();
  const cues = await queueCues(ids);
  const index = cues.findIndex((c) => c.id === startId);
  if (index < 0) throw new Error('track not found');
  quiet();
  expect(startId);
  stagedNextId = null;
  await nativeSetQueue({ items: cues.map(toQueueItem), index, play: true, repeat, autoplay });
}

/// Replaces the queue around what is playing, without touching it: shuffle
/// toggled, or levels/trims that changed with a preference.
export async function updateQueue(
  ids: number[],
  currentId: number | null,
  repeat: string,
  autoplay: boolean,
) {
  await ensureInitialized();
  const cues = await queueCues(ids);
  const index = currentId == null ? -1 : cues.findIndex((c) => c.id === currentId);
  await nativeSetQueue({ items: cues.map(toQueueItem), index, play: false, repeat, autoplay });
}

export async function setQueueMode(repeat: string, autoplay: boolean) {
  await ensureInitialized();
  await nativeSetQueueMode(repeat, autoplay);
}

export async function nextTrack() {
  restoreCancelled = true;
  await ensureInitialized();
  quiet();
  await queueNext();
}

/// Previous entry, or the start of this one once it has played a while —
/// decided by the player, which knows the position without a round trip.
export async function prevTrack() {
  restoreCancelled = true;
  await ensureInitialized();
  quiet();
  await queuePrev();
}

export async function playTrack(id: number) {
  restoreCancelled = true;
  await ensureInitialized();
  // Already staged and there's an overlap configured: this is the transition
  // the plugin was set up for, so let it fade instead of cutting.
  if (id === stagedNextId && crossfadeSecs > 0) {
    expect(id);
    const st = await skipToNext();
    if (st.skipped) {
      quiet();
      return;
    }
    // Nothing was staged after all; fall through and load it the plain way.
  }
  const cue = await trackCue(id);
  quiet();
  expect(id);
  stagedNextId = null;
  await setSource({ ...toQueueItem(cue), id });
  await play();
}

/// What plays after the current track, or `null` to clear it.
///
/// With crossfade off the plugin appends it to the same ExoPlayer's playlist,
/// which is where the gapless transition comes from; with crossfade on it
/// prepares a second player and starts it early. Either way the track change
/// arrives as a `advanced` event instead of `ended` + a new `playTrack`.
export async function setNextTrack(id: number | null) {
  await ensureInitialized();
  if (id == null) {
    if (stagedNextId == null) return;
    stagedNextId = null;
    stagingInFlight++;
    try {
      await setNextSource(null);
    } finally {
      stagingInFlight--;
    }
    return;
  }
  if (id === stagedNextId) return;
  const cue = await trackCue(id);
  stagedNextId = id;
  stagingInFlight++;
  try {
    await setNextSource({ ...toQueueItem(cue), id });
  } finally {
    stagingInFlight--;
  }
}

/// Re-reads a track's level and applies it if that track is the one playing.
///
/// The desktop command does this to its own player from Rust; here the player
/// is the plugin, which Rust can't reach. A staged track is left alone: it
/// gets its gain from `setNextTrack` when it is re-staged, and the app
/// re-stages on every change to the queue.
export async function refreshGain(id: number) {
  if (lastTrackId !== id) return;
  const cue = await trackCue(id);
  await setSourceGain(cue.gainDb);
}

export async function pausePlayback() {
  await pause();
}

export async function resumePlayback() {
  await play();
}

export async function stopPlayback() {
  restoreCancelled = true;
  quiet();
  lastTrackId = null;
  awaitedId = null;
  stagedNextId = null;
  // The player clears everything itself — queue, staged next, checkpoint and
  // notification — the same as when stop comes from the notification (3.3).
  await nativeStop();
}

export async function seekTo(secs: number) {
  quiet();
  await nativeSeekTo(secs);
}

export async function playbackPosition(): Promise<number> {
  const st = await getState();
  return Math.floor(st.currentTime || 0);
}

/// Master volume, 0..1. Applied by `ExoPlayer.setVolume` on whichever deck is
/// active, on top of the track's own gain.
export async function setVolume(volume: number) {
  await ensureInitialized();
  await nativeSetVolume(volume);
}

/// Overlap between one track and the next, in seconds. `0` turns it off, and
/// that is also what makes the transition gapless: with no overlap the staged
/// track goes into the same player's playlist.
export async function setCrossfade(seconds: number) {
  await ensureInitialized();
  crossfadeSecs = seconds;
  await nativeSetCrossfade(seconds);
}

export async function listOutputDevices(): Promise<NativeAudioOutputDevice[]> {
  await ensureInitialized();
  return nativeListOutputDevices();
}

/// The devices as names, which is the currency the rest of the app deals in
/// (`PlaybackPrefs.outputDevice`) because desktop's cpal only has names and
/// because Android's ids don't survive a reconnect.
///
/// The kind is folded into the name when it adds anything: two "SM-S926B"
/// entries that differ only by being the speaker and the earpiece are not a
/// choice anyone can make.
export async function listOutputDeviceNames(): Promise<string[]> {
  const devices = await listOutputDevices();
  return devices.map((d) => (d.name === d.type ? d.name : `${d.name} (${d.type})`));
}

/// `null` hands the choice back to Android, which is also what happens if the
/// pinned device is unplugged — better than going silent.
export async function setOutputDevice(id: number | null) {
  await ensureInitialized();
  await nativeSetOutputDevice(id);
}

/// Pins the device with this name, resolving it to the id the plugin wants.
///
/// A name that isn't in the list any more resolves to `null`: the headphones
/// are unplugged, and Android's own routing is a better answer than silence.
/// The preference is left alone so plugging them back in re-pins them.
export async function setOutputDeviceByName(name: string | null) {
  if (name == null) return setOutputDevice(null);
  const devices = await listOutputDevices();
  const match = devices.find(
    (d) => (d.name === d.type ? d.name : `${d.name} (${d.type})`) === name,
  );
  await setOutputDevice(match?.id ?? null);
}

// --- State pushed by the plugin ---------------------------------------------
//
// The plugin emits its state on every tick (25ms in foreground, 250ms in
// background). Listening to it instead of polling from JS gives three things
// polling doesn't: a real track end (`status: 'ended'`) instead of guessing
// it by comparing position against duration, play/pause toggled from the
// notification or the lockscreen, and — since the fork — the id of the track
// that is playing, which is the only sign that a queued one took over.
//
// For this to stay alive with the app in the background, `MainActivity.kt`
// prevents the WebView from pausing (see the reason there).

export type PlaybackEvent =
  | { type: 'position'; ms: number }
  | { type: 'playing'; value: boolean }
  /// A staged track took over on its own (gapless or crossfaded). There is no
  /// `ended` before it: the player never stopped.
  | { type: 'advanced'; id: number }
  | { type: 'ended' }
  /// Stopped from outside the app (the notification's stop button).
  | { type: 'stopped' }
  /// The player's repeat mode changed on its own ("once" consumed).
  | { type: 'repeat'; value: 'off' | 'track' | 'once' };

let endedSent = false;

/// How long before the end the track is switched while the app is off screen.
///
/// When the player reaches `STATE_ENDED`, the plugin's notification stops
/// being "ongoing" and its service LEAVES foreground. Starting the next track
/// brings it back, and `Service.startForeground()` from background is
/// forbidden on Android 12+: it crashes with
/// ForegroundServiceStartNotAllowedException. By switching tracks while the
/// current one is still playing, the notification never stops being
/// "ongoing", the service never leaves foreground, and the problem doesn't
/// exist — it's the same path that already works when tapping "next" on the
/// notification.
///
/// Only applies with the app off screen: in foreground the process has
/// plenty of permission and there's no need to trim the track at all. The
/// margin has to cover a couple of plugin ticks, which in background are 250ms.
///
/// It also only applies when nothing is staged. With a queued track the player
/// moves on by itself and never reaches `STATE_ENDED` in between, so there is
/// no moment when the service could drop out of foreground — that whole class
/// of crash stops existing.
const NEAR_END_LEAD_MS = 500;

let appVisible = true;
let endedEarly = false;

/// The app's real visibility, pushed by `MainActivity` (see the
/// `eval` in onPause/onResume). `document.visibilityState` isn't used because
/// MainActivity resumes the WebView by hand when the Activity pauses, so the
/// document keeps reporting itself as visible.
export function setAppVisible(visible: boolean) {
  appVisible = visible;
}

/// Discards the derived state after a deliberate position change (seek,
/// stop, new track).
function quiet() {
  endedSent = false;
  endedEarly = false;
  // The next position is a new one, not the continuation of the throttled
  // stream, so it goes out immediately instead of waiting out the rest of a
  // window that belonged to where we just were.
  lastPositionEmit = 0;
}

/// Translates the plugin's stream of states into events that make sense for
/// the app. `emit` receives position at most every `POSITION_MS` (the native
/// tick is too fast to put into React state), but seeks, transitions and track
/// end fire instantly.
const POSITION_MS = 200;
let lastPositionEmit = 0;

let lastReportedPlaying: boolean | null = null;
let lastReportedRepeat: string | null = null;

function handleState(st: NativeAudioState, emit: (e: PlaybackEvent) => void) {
  const now = Date.now();
  const ms = Math.round((st.currentTime || 0) * 1000);
  const id = st.trackId ?? null;

  if (awaitedId != null) {
    if (id === awaitedId) {
      awaitedId = null;
      lastTrackId = id;
    } else if (now < awaitedUntil) {
      // Still describing the track we're leaving. Reporting any of it — the
      // old position most of all — would make the bar jump backwards.
      return;
    } else {
      // The change never arrived. Take whatever is playing as the truth
      // rather than emitting a transition nobody asked for.
      awaitedId = null;
      lastTrackId = id;
    }
  } else if (id == null && lastTrackId != null) {
    // The player has nothing loaded any more and the app didn't ask for it
    // (its own stop clears `lastTrackId` first): stopped from the
    // notification.
    lastTrackId = null;
    quiet();
    emit({ type: 'stopped' });
    return;
  } else if (id != null && id !== lastTrackId) {
    // A queued track took over. Nothing ended and nothing was asked for, so
    // this is the only place the change shows up.
    const first = lastTrackId == null;
    lastTrackId = id;
    quiet();
    if (!first) {
      emit({ type: 'advanced', id });
      lastPositionEmit = 0;
    }
  }

  // What the plugin says is staged wins over what was requested — a track that
  // failed to prepare is dropped there, and the app has to hear about it to
  // queue another one. Except while a staging call is still in flight, when
  // this is by definition the answer to the previous question.
  if (stagingInFlight === 0) stagedNextId = st.nextTrackId ?? null;
  if (typeof st.crossfade === 'number') crossfadeSecs = st.crossfade;

  if (st.repeatMode && st.repeatMode !== lastReportedRepeat) {
    const first = lastReportedRepeat == null;
    lastReportedRepeat = st.repeatMode;
    // The first state only says what the app itself set; reporting it back
    // would be an echo.
    if (!first) emit({ type: 'repeat', value: st.repeatMode });
  }
  const nativeQueue = (st.queueLength ?? 0) > 0;

  // Play/pause can also be toggled from the notification and the lockscreen,
  // not just from the app: without this, the app's button keeps showing a
  // state that's no longer true.
  if (st.status !== 'ended' && st.isPlaying !== lastReportedPlaying) {
    lastReportedPlaying = st.isPlaying;
    emit({ type: 'playing', value: st.isPlaying });
  }

  if (st.status === 'ended') {
    if (!endedSent) {
      endedSent = true;
      emit({ type: 'ended' });
    }
    return;
  }
  endedSent = false;

  const durationMs = Math.round((st.duration || 0) * 1000);
  if (
    !appVisible &&
    !endedEarly &&
    stagedNextId == null &&
    // With a native queue nothing restarts after the end on its own (the
    // player stages what follows before it gets there), so there is no
    // foreground-service restart to dodge and no reason to cut the tail.
    !nativeQueue &&
    st.isPlaying &&
    durationMs > 0 &&
    durationMs - ms <= NEAR_END_LEAD_MS
  ) {
    endedEarly = true;
    emit({ type: 'ended' });
    return;
  }

  if (now - lastPositionEmit >= POSITION_MS) {
    lastPositionEmit = now;
    emit({ type: 'position', ms });
  }
}

export async function subscribePlayback(
  emit: (e: PlaybackEvent) => void,
): Promise<() => void> {
  await ensureInitialized();
  return addStateListener((st) => handleState(st, emit));
}
