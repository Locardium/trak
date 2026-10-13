import { useEffect, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { Modal } from './Modal';
import { pendingDeletes, resolvePendingDeletes, type PendingDeletes } from '../api';

interface Props {
  onStatus: (msg: string) => void;
  onLibraryChanged: () => void;
}

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

function describe(p: PendingDeletes): string {
  const parts = [];
  if (p.tracks) parts.push(plural(p.tracks, 'track', 'tracks'));
  if (p.playlists) parts.push(plural(p.playlists, 'playlist', 'playlists'));
  if (p.removals) parts.push(plural(p.removals, 'track removed from a playlist', 'tracks removed from playlists'));
  return parts.join(', ');
}

/// Deletes a device made while it wasn't linked to this one (2.7).
///
/// Re-linking is like linking a new device: what the other side deleted in
/// the meantime isn't applied on its own. The sync holds it and this asks.
/// Lives in App, like PairingDialog: the sync that holds them usually runs
/// in the background, with any screen open. Closing it without answering
/// leaves them held; it asks again on the next sync or app start.
export default function HeldDeletesDialog({ onStatus, onLibraryChanged }: Props) {
  const [held, setHeld] = useState<PendingDeletes | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!('__TAURI_INTERNALS__' in window)) return;
    const check = () =>
      pendingDeletes()
        .then((all) => setHeld(all[0] ?? null))
        .catch(() => {});
    check();
    const un = listen('pending-deletes', check);
    return () => {
      un.then((f) => f());
    };
  }, []);

  async function decide(keep: boolean) {
    if (!held) return;
    const who = held.deviceName || 'The other device';
    setBusy(true);
    try {
      await resolvePendingDeletes(held.deviceUid, keep);
      onStatus(keep ? `Kept — sending them back to ${who}` : 'Deleted here too (in the trash for 30 days)');
      onLibraryChanged();
      const rest = await pendingDeletes();
      setHeld(rest[0] ?? null);
    } catch (e) {
      onStatus(String(e));
    } finally {
      setBusy(false);
    }
  }

  if (!held) return null;
  const who = held.deviceName || 'A device';

  return (
    <Modal key={held.deviceUid} title={`${who} deleted music you have here`} onClose={() => setHeld(null)}>
      <div className="pair-dialog">
        <p className="set-note">
          While it wasn't linked to this device, <strong>{who}</strong> deleted {describe(held)} that
          you still have here. Nothing was deleted on this device.
        </p>
        <div className="pair-actions">
          <button onClick={() => decide(false)} disabled={busy}>
            Delete here too
          </button>
          <button className="primary" onClick={() => decide(true)} disabled={busy}>
            Keep and send back
          </button>
        </div>
      </div>
    </Modal>
  );
}
