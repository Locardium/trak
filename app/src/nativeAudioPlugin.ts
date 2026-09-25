// JS side of `crates/native-audio`, Sway's fork of `tauri-plugin-native-audio`
// (see its FORK.md). It replaces the `tauri-plugin-native-audio-api` package:
// the fork adds commands that package doesn't know about, and publishing a
// package just to import six more `invoke` calls isn't worth it.
//
// Command names go over the wire in snake_case and land on lowerCamelCase
// Kotlin methods — tauri runs them through `heck::AsLowerCamelCase` before
// handing them to the Android plugin. `permissions/` on the Rust side uses the
// snake_case spelling, so both have to match what's below.
import { addPluginListener, invoke } from '@tauri-apps/api/core';

const PLUGIN_NAME = 'native-audio';
const STATE_EVENT = 'native_audio_state';

const call = <T>(command: string, payload?: Record<string, unknown>) =>
  invoke<T>(`plugin:${PLUGIN_NAME}|${command}`, payload);

export type NativeAudioStatus = 'idle' | 'loading' | 'playing' | 'ended' | 'error';

export interface NativeAudioState {
  status: NativeAudioStatus;
  currentTime: number;
  duration: number;
  isPlaying: boolean;
  buffering: boolean;
  rate: number;
  /// Master volume, 0..1. Independent of `gainDb`, which is per track.
  volume: number;
  /// Gain of the track playing right now, in dB (ReplayGain/R128).
  gainDb: number;
  /// Overlap between tracks, in seconds. 0 = off, which is what makes the
  /// transition gapless instead.
  crossfade: number;
  /// Id of the track playing right now. When a queued track starts there is no
  /// `ended` in between — this changing is the only signal.
  trackId?: number;
  /// Id of the track staged behind it, if any.
  nextTrackId?: number;
  outputDeviceId?: number;
  error?: string;
  /// Native queue (Fase 1.7). "once" turns itself into "off" in the player,
  /// on the transition, so the app reads it back from here.
  repeatMode?: 'off' | 'track' | 'once';
  /// -1 with no queue.
  queueIndex?: number;
  queueLength?: number;
}

/// One entry of the native queue: what the player needs to play it with no
/// help from JS — it has to, when the app is off screen or was killed.
export interface NativeAudioQueueItem {
  id: number;
  src: string;
  title?: string;
  artist?: string;
  artwork?: string;
  gainDb?: number;
  leadMs?: number;
  audioEndMs?: number;
}

export interface NativeAudioSource {
  src: string;
  id?: number;
  title?: string;
  artist?: string;
  /// Cached cover thumb, a file path. Shown by the notification.
  artwork?: string;
  /// Per-track level in dB, cut or boost. The cut half rides `ExoPlayer.volume`
  /// and the boost half a `LoudnessEnhancer`, because that volume saturates at
  /// 1.0 — see `applyVolumeLocked` in the plugin.
  gainDb?: number;
  /// Playable region in ms, from the analyzer. The silence a file carries at
  /// its edges is what the gap between two tracks is made of, so playing only
  /// between these two is what makes the handover gapless. `0` for either
  /// means the file's own edge.
  leadMs?: number;
  audioEndMs?: number;
}

export interface NativeAudioOutputDevice {
  id: number;
  /// Readable kind ("Bluetooth", "Wired headphones", …), from AudioDeviceInfo.
  type: string;
  name: string;
}

export interface NativeAudioProgressCheckpoint {
  id: number;
  currentTime: number;
  updatedAtMs: number;
  status?: NativeAudioStatus;
}

export const initialize = () => call<NativeAudioState>('initialize');
export const setSource = (payload: NativeAudioSource) =>
  call<NativeAudioState>('set_source', payload as unknown as Record<string, unknown>);
export const play = () => call<NativeAudioState>('play');
export const pause = () => call<NativeAudioState>('pause');
export const seekTo = (position: number) => call<NativeAudioState>('seek_to', { position });
export const setRate = (rate: number) => call<NativeAudioState>('set_rate', { rate });
export const getState = () => call<NativeAudioState>('get_state');
export const getProgressCheckpoint = () =>
  call<NativeAudioProgressCheckpoint | null>('get_progress_checkpoint');
export const clearProgressCheckpoint = () => call<void>('clear_progress_checkpoint');
/// Stop: the player drops the queue, the checkpoint and the notification.
export const stop = () => call<void>('stop');
export const dispose = () => call<void>('dispose');

// --- Added by the fork ------------------------------------------------------

export const setVolume = (volume: number) => call<NativeAudioState>('set_volume', { volume });

/// Replaces the gain of the track already playing, without restarting it.
export const setSourceGain = (gainDb: number) =>
  call<NativeAudioState>('set_source_gain', { gainDb });

/// What plays after the current track, or `null` to clear it. With crossfade
/// off this goes into the same ExoPlayer's playlist (gapless); with crossfade
/// on it's prepared on a second player and started early.
export const setNextSource = (payload: NativeAudioSource | null) =>
  call<NativeAudioState>('set_next_source', (payload ?? {}) as unknown as Record<string, unknown>);

/// Starts the staged track now. `skipped` is false when nothing was staged —
/// the caller then has to fall back to `setSource`.
export const skipToNext = () =>
  call<NativeAudioState & { skipped: boolean }>('skip_to_next');

export const setCrossfade = (seconds: number) =>
  call<NativeAudioState>('set_crossfade', { seconds });

export const listOutputDevices = async (): Promise<NativeAudioOutputDevice[]> => {
  const res = await call<{ devices: NativeAudioOutputDevice[] }>('list_output_devices');
  return res.devices ?? [];
};

/// `null` gives the choice back to Android. An id that no longer exists does
/// the same rather than going silent.
export const setOutputDevice = (id: number | null) =>
  call<NativeAudioState>('set_output_device', { id });

/// Returns the unsubscribe. `addPluginListener` resolves to a `PluginListener`
/// object, not to a function — the published package typed it as one, which
/// meant unsubscribing threw. Wrapped here so callers get what the type says.
export const addStateListener = async (
  handler: (state: NativeAudioState) => void,
): Promise<() => void> => {
  const listener = await addPluginListener(PLUGIN_NAME, STATE_EVENT, handler);
  return () => {
    void listener.unregister();
  };
};

// --- Battery-optimization exemption (Fase 1.3) ------------------------------
//
// Not audio playback per se, but the same reason everything else in this file
// exists: Android can freeze or kill the process this plugin's foreground
// service runs in while it's backgrounded and unexempt, and that's what makes
// the notification / lockscreen / a car's Bluetooth stop advancing tracks
// until the app is reopened by hand (see `NativeAudioRuntime.transport`).

export const isIgnoringBatteryOptimizations = async (): Promise<boolean> => {
  const res = await call<{ ignoring: boolean }>('is_ignoring_battery_optimizations');
  return res.ignoring;
};

/// Opens the system's own exemption dialog; there's no silent way to ask.
/// Resolves once the dialog is shown, not once the user answers — the caller
/// re-reads `isIgnoringBatteryOptimizations` when the app is next in front.
export const requestIgnoreBatteryOptimizations = () =>
  call<void>('request_ignore_battery_optimizations');

// --- Native play queue (Fase 1.7) --------------------------------------------
//
// The player owns the queue: next/previous/end of track are decided on the
// native side, so a button pressed with the app off screen (where React stops
// re-rendering) or after Android killed the process still does the right
// thing. JS builds the queue and hands it over.

/// `play: true` loads `index` and starts it. `play: false` only replaces the
/// queue around what is playing (shuffle toggled, levels refreshed).
export const setQueue = (payload: {
  items: NativeAudioQueueItem[];
  index: number;
  play: boolean;
  repeat?: string;
  autoplay?: boolean;
}) => call<NativeAudioState>('set_queue', payload as unknown as Record<string, unknown>);

export const setQueueMode = (repeat?: string, autoplay?: boolean) =>
  call<NativeAudioState>('set_queue_mode', { repeat, autoplay });

export const queueNext = () => call<NativeAudioState>('queue_next');
export const queuePrev = () => call<NativeAudioState>('queue_prev');

/// Reads back the queue an earlier process saved, once per process; in a
/// fresh one, also loads its current entry, paused where it stopped.
export const restoreQueue = () => call<NativeAudioState>('restore_queue');
