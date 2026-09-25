import { useEffect, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { Modal } from './Modal';
import { confirmPairing, type PairingDone, type PairingRequest } from '../api';

interface Props {
  onStatus: (msg: string) => void;
}

/// The pairing code, on top of whatever screen is open.
///
/// It lives in App and not in Sync: a request reaches a device that may be
/// anywhere — the library, Settings — and the event fires only once. With
/// the listener inside Sync, a device that wasn't on that screen never showed
/// the code, and the one that asked waited for nothing (Fase 1.9).
///
/// It's also its own Modal on purpose. It used to be returned by Sync in
/// place of Sync's own Modal: React reused the same instance, so closing the
/// code with the X left the `closing` state on Sync's panel, whose invisible
/// backdrop then swallowed every click — the "frozen" UI.
export default function PairingDialog({ onStatus }: Props) {
  const [pairing, setPairing] = useState<PairingRequest | null>(null);

  useEffect(() => {
    if (!('__TAURI_INTERNALS__' in window)) return;
    const uns = [
      listen<PairingRequest>('pairing-request', (e) => setPairing(e.payload)),
      listen<PairingDone>('pairing-done', (e) => {
        const { uid, name, ok, error } = e.payload;
        // Only the request on screen: a stale one ending must not close a
        // newer dialog.
        setPairing((p) => (p && p.uid === uid ? null : p));
        onStatus(ok ? `Paired with ${name}` : `${name}: ${error ?? 'pairing failed'}`);
      }),
    ];
    return () => uns.forEach((u) => u.then((f) => f()));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function decide(accept: boolean) {
    if (!pairing) return;
    const uid = pairing.uid;
    setPairing(null);
    try {
      await confirmPairing(uid, accept);
    } catch (e) {
      onStatus(String(e));
    }
  }

  if (!pairing) return null;

  // The code goes above everything else: it's a security decision and can't
  // stay hidden behind a scroll.
  return (
    <Modal
      // A new request is a new dialog, never the leftover of the last one.
      key={`${pairing.uid}:${pairing.code}`}
      title={pairing.incoming ? `${pairing.name} wants to pair` : `Pair with ${pairing.name}`}
      onClose={() => decide(false)}
    >
      <div className="pair-dialog">
        <p className="set-note">
          Check that this code is showing on <strong>{pairing.name}</strong> too. If the two codes
          are different, someone else is on the line — reject it.
        </p>
        <div className="pair-code">{pairing.code}</div>
        <div className="pair-actions">
          <button onClick={() => decide(false)}>Reject</button>
          <button className="primary" onClick={() => decide(true)}>
            Codes match
          </button>
        </div>
      </div>
    </Modal>
  );
}
