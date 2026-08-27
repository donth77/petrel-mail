import { useCallback, useEffect, useRef, useState } from 'react';

/**
 * Makes a fixed-position panel draggable by a handle, without disturbing how
 * it is anchored.
 *
 * The composer is pinned to the bottom-right corner, and that is where it
 * should reappear next time. So this moves it with a transform rather than by
 * writing `inset`: the anchoring stays exactly as the stylesheet wrote it, and
 * the offset is a separate thing that can be thrown away.
 *
 * The panel is kept entirely on screen. Letting a window be dragged half off
 * the edge is a convention borrowed from desktops that have a title bar you
 * can always grab back; this one can be dragged somewhere with nothing left to
 * grab, and then the only way back is to close it.
 */
export function useDragWindow() {
  const ref = useRef<HTMLElement | null>(null);
  const [offset, setOffset] = useState({ x: 0, y: 0 });
  // Where the pointer was, and what the offset was, when the drag began.
  const from = useRef<{ px: number; py: number; ox: number; oy: number } | null>(null);

  /** How far the panel may move from where the stylesheet put it. */
  const limits = useCallback(() => {
    const el = ref.current;
    if (!el) return null;
    const r = el.getBoundingClientRect();
    // r already includes the current offset, so subtract it to get the
    // stylesheet's own position and measure the room around that.
    const left = r.left - offset.x;
    const top = r.top - offset.y;
    return {
      minX: -left,
      maxX: window.innerWidth - (left + r.width),
      minY: -top,
      maxY: window.innerHeight - (top + r.height),
    };
  }, [offset.x, offset.y]);

  const clamp = useCallback(
    (x: number, y: number) => {
      const l = limits();
      if (!l) return { x, y };
      return {
        // A panel taller or wider than the window has no room at all; min then
        // exceeds max, and Math.min/Math.max in this order pins it in place
        // rather than letting it jump.
        x: Math.min(Math.max(x, l.minX), Math.max(l.maxX, l.minX)),
        y: Math.min(Math.max(y, l.minY), Math.max(l.maxY, l.minY)),
      };
    },
    [limits],
  );

  const onPointerDown = useCallback(
    (e: React.PointerEvent) => {
      // Only the handle itself. A drag started on the close button would mean
      // the button never gets its click.
      if ((e.target as HTMLElement).closest('button, input, select, textarea, a')) return;
      if (e.button !== 0) return;
      from.current = { px: e.clientX, py: e.clientY, ox: offset.x, oy: offset.y };
      (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    },
    [offset.x, offset.y],
  );

  const onPointerMove = useCallback(
    (e: React.PointerEvent) => {
      const start = from.current;
      if (!start) return;
      e.preventDefault();
      setOffset(clamp(start.ox + (e.clientX - start.px), start.oy + (e.clientY - start.py)));
    },
    [clamp],
  );

  const onPointerUp = useCallback((e: React.PointerEvent) => {
    from.current = null;
    const el = e.currentTarget as HTMLElement;
    if (el.hasPointerCapture(e.pointerId)) el.releasePointerCapture(e.pointerId);
  }, []);

  // A window that shrinks can leave the panel outside it. Re-clamping on resize
  // is what stops a dragged composer from being lost by resizing the window.
  useEffect(() => {
    const onResize = () => setOffset((o) => clamp(o.x, o.y));
    window.addEventListener('resize', onResize);
    return () => window.removeEventListener('resize', onResize);
  }, [clamp]);

  return {
    ref,
    /** Spread onto the drag handle. */
    handleProps: { onPointerDown, onPointerMove, onPointerUp, onPointerCancel: onPointerUp },
    /** Spread onto the panel. */
    style:
      offset.x || offset.y
        ? ({ transform: `translate(${offset.x}px, ${offset.y}px)` } as const)
        : undefined,
    moved: offset.x !== 0 || offset.y !== 0,
  };
}
