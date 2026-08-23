import type React from 'react';
import { t } from '../lib/strings';

/**
 * The divider between the conversation list and the reading pane.
 *
 * A separate component rather than another strip inside one of the panes: it
 * belongs to neither, and putting it in either would make that pane responsible
 * for a width that is really the layout's.
 *
 * Given a `separator` role and a tab stop for the same reason the rail's is —
 * a layout you can only change by holding a mouse button down is a layout some
 * people cannot change at all.
 */
export function PaneResize({ onResize }: { onResize: (xOrDelta: number) => void }) {
  const startDrag = (e: React.PointerEvent) => {
    e.preventDefault();
    // Listeners on the window, not the handle: a fast drag outruns a 6px
    // target, and a pointer lost mid-drag would leave the panes at whatever
    // width the last event happened to land on.
    const move = (ev: PointerEvent) => onResize(ev.clientX);
    const up = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      document.body.classList.remove('resizing');
    };
    document.body.classList.add('resizing');
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  };

  return (
    <div
      className="pane-resize"
      role="separator"
      aria-orientation="vertical"
      aria-label={t('pane-resize')}
      tabIndex={0}
      onPointerDown={startDrag}
      onKeyDown={(e) => {
        const step = e.shiftKey ? 32 : 8;
        if (e.key === 'ArrowLeft') {
          e.preventDefault();
          onResize(-step);
        } else if (e.key === 'ArrowRight') {
          e.preventDefault();
          onResize(step);
        }
      }}
    />
  );
}
