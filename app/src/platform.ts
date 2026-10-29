import { platform } from '@tauri-apps/plugin-os';

// Reference phone width in CSS px (an S24 at the default display size is ~384).
const REF_WIDTH = 390;

// When the user shrinks Android's "display size", the same px UI gets tiny. Counter part of
// that: scale the root up by (width / REF_WIDTH)^0.6, never below 1 and capped, so the setting
// still matters but doesn't make everything minuscule. Done through the viewport meta (a
// narrower layout width, which the WebView scales to fit) rather than CSS `zoom`: zoom
// leaves vw/fixed layouts wider than the screen. screen.* is in CSS px and doesn't depend
// on the viewport meta, so there's no feedback loop.
export function applyAndroidScale(): void {
  if (!isAndroid()) return;
  const w = Math.min(window.screen.width, window.screen.height);
  if (!w) return;
  const scale = Math.min(1.35, Math.max(1, Math.pow(w / REF_WIDTH, 0.6)));
  if (scale <= 1.01) return;
  const layout = Math.round(window.screen.width / scale);
  const meta = document.querySelector('meta[name="viewport"]');
  meta?.setAttribute('content', `width=${layout}, user-scalable=no, viewport-fit=cover`);
}

// Outside Tauri (a regular browser in dev without a webview), platform() doesn't apply.
export const isAndroid = (): boolean => {
  if (!('__TAURI_INTERNALS__' in window)) return false;
  try {
    return platform() === 'android';
  } catch {
    return false;
  }
};
