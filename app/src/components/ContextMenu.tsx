import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { Check } from 'lucide-react';

export interface MenuItem {
  label: string;
  onClick?: () => void;
  danger?: boolean;
  disabled?: boolean;
  /** undefined = normal item; true/false = item with checkbox. */
  checked?: boolean;
  separator?: boolean;
}

interface Props {
  x: number;
  y: number;
  items: MenuItem[];
  onClose: () => void;
  /** With checkboxes, the menu doesn't close when an item is clicked. */
  keepOpen?: boolean;
}

export default function ContextMenu({ x, y, items, onClose, keepOpen }: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ x, y });
  const anyCheckable = items.some((i) => i.checked !== undefined);

  // Like a native menu: opens down and to the right of the cursor, and flips
  // up / left when that side doesn't have room, instead of sliding under it.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    const M = 8;
    const fit = (at: number, size: number, room: number) =>
      Math.max(M, at + size <= room - M ? at : at - size);
    setPos({ x: fit(x, r.width, window.innerWidth), y: fit(y, r.height, window.innerHeight) });
  }, [x, y]);

  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) onClose();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('mousedown', onDown);
    window.addEventListener('keydown', onKey);
    window.addEventListener('blur', onClose);
    return () => {
      window.removeEventListener('mousedown', onDown);
      window.removeEventListener('keydown', onKey);
      window.removeEventListener('blur', onClose);
    };
  }, [onClose]);

  // Portaled to <body>: inside a transformed or scrolling ancestor (the
  // sidebar drawer on mobile) `position: fixed` is relative to it and clipped.
  return createPortal(
    <div ref={ref} className="ctx-menu" style={{ left: pos.x, top: pos.y }} role="menu">
      {items.map((it, i) =>
        it.separator ? (
          <hr key={i} />
        ) : (
          <button
            key={i}
            role="menuitem"
            disabled={it.disabled}
            className={it.danger ? 'danger' : ''}
            onClick={() => {
              it.onClick?.();
              if (!keepOpen) onClose();
            }}
          >
            {/* If anything in this menu is checkable, every item reserves the
                check column — otherwise a plain item mixed in with checkboxes
                starts its label 24px to the left of all the others. */}
            {anyCheckable && (
              <span className="ctx-check">{it.checked ? <Check size={13} /> : null}</span>
            )}
            {it.label}
          </button>
        ),
      )}
    </div>,
    document.body,
  );
}
