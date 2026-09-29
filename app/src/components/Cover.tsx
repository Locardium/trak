import { useEffect, useRef, useState } from 'react';
import { Music } from 'lucide-react';
import { coverThumb } from '../api';

// Global cover cache by track id (data-URL or null if it has none).
const cache = new Map<number, string | null>();

/** Forgets the covers of tracks that are gone. Ids get reused after a
 *  delete, and a new track must not show the old one's cover. */
export function keepCovers(ids: Set<number>) {
  for (const id of cache.keys()) if (!ids.has(id)) cache.delete(id);
}

// --- Request queue ------------------------------------------------------------
// A fast scroll used to fire one `cover_thumb` per row it passed, hundreds at
// once, and the rows actually on screen waited behind all of them. Now at most
// a few are out at a time, the newest request goes first (it's the row the
// user is looking at), and a row that leaves before its turn is dropped.

const MAX_IN_FLIGHT = 4;
const waiting: number[] = [];
const inFlight = new Set<number>();
const listeners = new Map<number, Set<(v: string | null) => void>>();

function pump() {
  while (inFlight.size < MAX_IN_FLIGHT && waiting.length > 0) {
    const id = waiting.pop()!;
    inFlight.add(id);
    coverThumb(id)
      .catch(() => null)
      .then((v) => {
        cache.set(id, v);
        inFlight.delete(id);
        listeners.get(id)?.forEach((cb) => cb(v));
        listeners.delete(id);
        pump();
      });
  }
}

/** Asks for the cover of `id`; `cb` gets it once it's loaded. The returned
 *  function withdraws the request (a no-op once it has been answered). */
function want(id: number, cb: (v: string | null) => void): () => void {
  // Another view of the same track may have loaded it meanwhile.
  if (cache.has(id)) {
    cb(cache.get(id)!);
    return () => {};
  }
  let cbs = listeners.get(id);
  if (!cbs) listeners.set(id, (cbs = new Set()));
  cbs.add(cb);
  if (!inFlight.has(id)) {
    const i = waiting.indexOf(id);
    if (i >= 0) waiting.splice(i, 1);
    waiting.push(id);
    pump();
  }
  return () => {
    cbs.delete(cb);
    if (cbs.size > 0 || inFlight.has(id)) return;
    listeners.delete(id);
    const i = waiting.indexOf(id);
    if (i >= 0) waiting.splice(i, 1);
  };
}

// --- Visibility ------------------------------------------------------------------
// One observer per scroll container rather than one per row. The container has
// to be the root: with the viewport as root, rows below the table's visible
// area are clipped by its overflow and the margin never reaches them.

const LOOKAHEAD = '600px 0px';
const observers = new Map<Element | null, { io: IntersectionObserver; rows: number }>();
const onVisibility = new WeakMap<Element, (visible: boolean) => void>();

function observe(el: Element, cb: (visible: boolean) => void): () => void {
  const root = el.closest('.table-wrap');
  let obs = observers.get(root);
  if (!obs) {
    const io = new IntersectionObserver(
      (entries) => entries.forEach((e) => onVisibility.get(e.target)?.(e.isIntersecting)),
      { root, rootMargin: LOOKAHEAD },
    );
    observers.set(root, (obs = { io, rows: 0 }));
  }
  const o = obs;
  onVisibility.set(el, cb);
  o.io.observe(el);
  o.rows++;
  let done = false;
  return () => {
    if (done) return;
    done = true;
    o.io.unobserve(el);
    onVisibility.delete(el);
    // The table remounts on some view changes: don't keep its old root alive.
    if (--o.rows === 0) {
      o.io.disconnect();
      observers.delete(root);
    }
  };
}

interface Props {
  trackId: number | null;
  className?: string;
  /** true: loads immediately (player/panel). false: waits to be near the screen (rows). */
  eager?: boolean;
}

export default function Cover({ trackId, className, eager }: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const [src, setSrc] = useState<string | null>(
    trackId != null ? cache.get(trackId) ?? null : null,
  );
  const [loaded, setLoaded] = useState(trackId != null && cache.has(trackId));

  useEffect(() => {
    if (trackId == null) {
      setSrc(null);
      setLoaded(false);
      return;
    }
    if (cache.has(trackId)) {
      setSrc(cache.get(trackId)!);
      setLoaded(true);
      return;
    }
    setSrc(null);
    setLoaded(false);
    const apply = (v: string | null) => {
      setSrc(v);
      setLoaded(true);
    };
    if (eager) return want(trackId, apply);

    const el = ref.current;
    if (!el) return;
    let withdraw: (() => void) | null = null;
    const unobserve = observe(el, (visible) => {
      if (visible && !withdraw) {
        withdraw = want(trackId, (v) => {
          unobserve();
          apply(v);
        });
      } else if (!visible && withdraw) {
        withdraw();
        withdraw = null;
      }
    });
    return () => {
      unobserve();
      withdraw?.();
    };
  }, [trackId, eager]);

  return (
    <div ref={ref} className={'cover ' + (className ?? '')}>
      {src ? (
        <img src={src} alt="" draggable={false} />
      ) : (
        <Music size={loaded ? 16 : 14} className="cover-fallback" aria-hidden="true" />
      )}
    </div>
  );
}
