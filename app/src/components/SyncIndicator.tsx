import { useEffect, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { listPeers, syncRunPercent, type SyncDone, type SyncProgress } from '../api';

/// How often the toast repaints. `sync-progress` arrives per chunk — many per
/// second — and re-rendering on each one is part of what makes the phone lag
/// during a sync (1.2).
const PAINT_MS = 250;
/// No news for this long = the run is gone (a dropped connection may never
/// send `sync-done`): better to hide than to show a frozen bar forever.
const STALE_MS = 30_000;

interface Props {
  /// The Sync panel already draws the progress in each device's row.
  hidden: boolean;
}

/// Files moving between devices, visible from any screen. It used to show
/// only inside the Sync panel, so a sync started on its own (a new link,
/// auto-sync) moved hundreds of tracks without anything saying so.
export default function SyncIndicator({ hidden }: Props) {
  const [shown, setShown] = useState<SyncProgress | null>(null);
  const latest = useRef<SyncProgress | null>(null);
  const lastAt = useRef(0);
  const names = useRef<Record<string, string>>({});
  const [, setNamesVersion] = useState(0);

  useEffect(() => {
    if (!('__TAURI_INTERNALS__' in window)) return;
    const timer = setInterval(() => {
      if (latest.current && Date.now() - lastAt.current > STALE_MS) latest.current = null;
      setShown((prev) => (prev === latest.current ? prev : latest.current));
    }, PAINT_MS);
    const uns = [
      listen<SyncProgress>('sync-progress', (e) => {
        const uid = e.payload.uid;
        latest.current = e.payload;
        lastAt.current = Date.now();
        if (!(uid in names.current)) {
          names.current[uid] = '';
          listPeers()
            .then((peers) => {
              for (const p of peers) names.current[p.uid] = p.name;
              setNamesVersion((v) => v + 1);
            })
            .catch(() => {});
        }
      }),
      listen<SyncDone>('sync-done', (e) => {
        if (latest.current?.uid === e.payload.uid) latest.current = null;
        setShown(latest.current);
      }),
      // Also on the side that only served the run (the other device
      // started it), where `sync-done` never comes.
      listen<string>('sync-idle', (e) => {
        if (latest.current?.uid === e.payload) latest.current = null;
        setShown(latest.current);
      }),
    ];
    return () => {
      clearInterval(timer);
      uns.forEach((u) => u.then((f) => f()));
    };
  }, []);

  if (!shown || hidden) return null;
  const name = names.current[shown.uid] || 'device';
  const pct = syncRunPercent(shown);
  return (
    <div className="import-toast sync-toast" role="status">
      <div className="import-toast-head">
        <span>
          {shown.sending ? `Sending to ${name}` : `Receiving from ${name}`}
        </span>
        <span className="import-toast-count">
          {shown.fileIndex} / {shown.fileTotal}
        </span>
      </div>
      <div className="sync-toast-file">{shown.filename}</div>
      <div className="import-bar">
        <div className="import-bar-fill" style={{ width: `${pct}%` }} />
      </div>
    </div>
  );
}
