import { useEffect, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { Modal } from './Modal';
import { resolveScopePrompt, scopePromptPending } from '../api';

interface Props {
  onStatus: (msg: string) => void;
  /// "Choose playlists": the Sync screen is where they're ticked.
  onChoose: () => void;
}

/// "What should this device sync?", asked when this device links while it
/// was linked to nothing (Fase 1.8; on re-linking too since 2026-10-13).
///
/// In App and on top of everything, like PairingDialog: the link finishes
/// wherever the user happens to be, and a row inside the Sync screen was easy
/// to miss — meanwhile the scope reads "selected", so no music comes in.
/// Closing it without answering leaves the question pending: it shows again
/// on the next link event or app start.
export default function ScopePromptDialog({ onStatus, onChoose }: Props) {
  const [open, setOpen] = useState(false);

  useEffect(() => {
    if (!('__TAURI_INTERNALS__' in window)) return;
    const check = () =>
      scopePromptPending()
        .then(setOpen)
        .catch(() => {});
    check();
    const uns = [listen('pairing-done', check), listen('peers-changed', check)];
    return () => uns.forEach((u) => u.then((f) => f()));
  }, []);

  async function answer(everything: boolean) {
    setOpen(false);
    try {
      await resolveScopePrompt(everything);
      if (everything) onStatus('Syncing the whole library');
      else onChoose();
    } catch (e) {
      onStatus(String(e));
    }
  }

  if (!open) return null;

  return (
    <Modal title="What should this device sync?" onClose={() => setOpen(false)}>
      <div className="pair-dialog">
        <p className="set-note">
          Playlists from the linked device are coming in now. No music is downloaded until you
          choose.
        </p>
        <div className="pair-actions">
          <button onClick={() => answer(false)}>Choose playlists</button>
          <button className="primary" onClick={() => answer(true)}>
            Everything
          </button>
        </div>
      </div>
    </Modal>
  );
}
