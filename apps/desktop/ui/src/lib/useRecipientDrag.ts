import { useCallback, useEffect, useRef } from 'react';
import { DRAG_THRESHOLD } from './dnd';
import { pressEnded, startsDrag } from './press';
import type { RecipientField, RecipientMove } from './recipients';

/**
 * Drags a confirmed recipient chip between To, Cc and Bcc.
 *
 * Not the list's drag. That one files conversations, and a shared drop target
 * would let a chip fall into a folder. Not the window drag either: that one
 * captures the pointer and keeps its offset in React state. This listens only
 * while the button is down, moves the preview through the DOM, and reports
 * the move once, when the chip is put down.
 */

type Point = { x: number; y: number };

type Press = {
  field: RecipientField;
  index: number;
  addr: string;
  x: number;
  y: number;
  pointerId: number;
  /** The pointer has travelled far enough that this is a drag, not a click. */
  live: boolean;
};

type Gesture = {
  move: (e: PointerEvent) => void;
  up: (e: PointerEvent) => void;
  cancel: (e: PointerEvent) => void;
  key: (e: KeyboardEvent) => void;
  finish: (drop: boolean, point?: Point) => void;
};

export function useRecipientDrag(
  section: React.RefObject<HTMLElement | null>,
  onMove: (move: RecipientMove) => void,
): (
  event: React.PointerEvent,
  field: RecipientField,
  index: number,
  addr: string,
) => void {
  const onMoveRef = useRef(onMove);
  onMoveRef.current = onMove;
  // The section object is what the composer rendered with. Reading it at
  // event time, not at the moment the listeners were built, follows a ref
  // that was filled in after this hook's first render.
  const sectionHolder = useRef(section);
  sectionHolder.current = section;

  const press = useRef<Press | null>(null);
  const overEl = useRef<HTMLElement | null>(null);
  const previewEl = useRef<HTMLDivElement | null>(null);
  const listening = useRef(false);
  const gesture = useRef<Gesture | null>(null);

  if (gesture.current === null) {
    const root = () => sectionHolder.current.current;

    /** The recipient row or hidden-field button under the pointer, if it is
     *  inside this composer. The button sits inside the To row, so the
     *  nearest marked element wins and the button's field beats the row's. */
    const targetAt = (x: number, y: number): HTMLElement | null => {
      const host = root();
      if (!host) return null;
      const el = document.elementFromPoint(x, y);
      if (!(el instanceof Element) || !host.contains(el)) return null;
      const hit = el.closest<HTMLElement>('[data-recipient-drop]');
      if (!hit || !host.contains(hit)) return null;
      return hit;
    };

    const unlisten = () => {
      const g = gesture.current;
      if (!g || !listening.current) return;
      listening.current = false;
      window.removeEventListener('pointermove', g.move);
      window.removeEventListener('pointerup', g.up);
      window.removeEventListener('pointercancel', g.cancel);
      window.removeEventListener('keydown', g.key);
    };

    const clearOver = () => {
      overEl.current = null;
      const host = root();
      if (!host) return;
      host.querySelectorAll<HTMLElement>('[data-drop-over]').forEach((el) => {
        el.removeAttribute('data-drop-over');
      });
    };

    const finish = (drop: boolean, point?: Point) => {
      const current = press.current;
      if (!current && !listening.current && !previewEl.current) return;
      press.current = null;
      unlisten();
      // Only a drag that crossed the threshold ever set the class. A click
      // must not take it off a drag the list is in the middle of.
      if (current?.live) document.body.classList.remove('dragging');
      const hit = drop && current?.live && point ? targetAt(point.x, point.y) : null;
      clearOver();
      const preview = previewEl.current;
      previewEl.current = null;
      preview?.remove();
      if (!current?.live || !hit) return;
      const to = hit.dataset.recipientDrop;
      if (to !== 'to' && to !== 'cc' && to !== 'bcc') return;
      onMoveRef.current({
        from: current.field,
        to,
        index: current.index,
        addr: current.addr,
      });
    };

    const place = (x: number, y: number) => {
      const preview = previewEl.current;
      if (!preview) return;
      // Beside the pointer, as the list's preview sits, so the carried chip
      // is not the element the next hit test would find. The class already
      // ignores the pointer; the offset keeps the words readable.
      preview.style.transform = `translate3d(${x + 12}px, ${y + 10}px, 0)`;
    };

    const hover = (x: number, y: number) => {
      const hit = targetAt(x, y);
      if (hit === overEl.current) return;
      if (overEl.current) overEl.current.removeAttribute('data-drop-over');
      overEl.current = hit;
      const preview = previewEl.current;
      if (hit) {
        hit.setAttribute('data-drop-over', 'true');
        preview?.setAttribute('data-over', 'true');
      } else {
        preview?.removeAttribute('data-over');
      }
    };

    const move = (e: PointerEvent) => {
      const current = press.current;
      if (!current || e.pointerId !== current.pointerId) return;
      // The button is already up. The list drag ignores the first such move,
      // so the pointerup after it can still drop, and gives up only on a
      // second one. Waiting here dropped the chip on the floor: that first
      // move is the release this window heard, and the chip lands where it
      // was. A press that never became a drag lands nowhere.
      if (pressEnded(e)) {
        finish(true, { x: e.clientX, y: e.clientY });
        return;
      }
      if (!current.live) {
        if (Math.hypot(e.clientX - current.x, e.clientY - current.y) < DRAG_THRESHOLD) return;
        current.live = true;
        document.body.classList.add('dragging');
        const preview = document.createElement('div');
        preview.className = 'drag-preview';
        preview.setAttribute('aria-hidden', 'true');
        preview.textContent = current.addr;
        document.body.appendChild(preview);
        previewEl.current = preview;
      }
      e.preventDefault();
      place(e.clientX, e.clientY);
      hover(e.clientX, e.clientY);
    };

    const up = (e: PointerEvent) => {
      const current = press.current;
      if (!current || e.pointerId !== current.pointerId) return;
      finish(true, { x: e.clientX, y: e.clientY });
    };

    // The pointer was taken away. That is not a place the chip was set down.
    const cancel = (e: PointerEvent) => {
      const current = press.current;
      if (!current || e.pointerId !== current.pointerId) return;
      finish(false);
    };

    // Escape puts the chip back, including before the pointer has moved far
    // enough to count as a drag. A press that is still a click is a press.
    const key = (e: KeyboardEvent) => {
      if (e.key !== 'Escape' || !press.current) return;
      finish(false);
    };

    gesture.current = { move, up, cancel, key, finish };
  }

  useEffect(() => () => gesture.current?.finish(false), []);

  return useCallback((event, field, index, addr) => {
    if (!startsDrag(event)) return;
    // The × that removes a chip is a button inside the chip. A press there
    // is a click, and starting a drag from it would swallow that click.
    const raw = event.target;
    if (raw instanceof Element && raw.closest('button')) return;
    const g = gesture.current;
    if (!g) return;
    if (press.current) g.finish(false);
    press.current = {
      field,
      index,
      addr,
      x: event.clientX,
      y: event.clientY,
      pointerId: event.pointerId,
      live: false,
    };
    listening.current = true;
    window.addEventListener('pointermove', g.move);
    window.addEventListener('pointerup', g.up);
    window.addEventListener('pointercancel', g.cancel);
    window.addEventListener('keydown', g.key);
  }, []);
}
