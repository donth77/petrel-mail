import { useCallback, useEffect, useRef, useState } from 'react';
import { DRAG_THRESHOLD, acceptsDrop, draggableFrom, draggedIds } from './dnd';

/**
 * What is being carried.
 *
 * Two directions, because tagging is a thing you can want to do from either
 * end: conversations onto a tag when you are working through a list, and a tag
 * onto a conversation when you are looking at the tag and know where it goes.
 */
export type Payload =
  | { kind: 'threads'; ids: number[] }
  | { kind: 'tag'; tagId: number; name: string };

/** Where the pointer is and what it is carrying, while a drag is happening. */
export type Dragging = {
  payload: Payload;
  /** What the preview should say. */
  label: string;
  x: number;
  y: number;
  /** The rail destination under the pointer, if it will accept the drop. */
  over: string | null;
  /** The conversation under the pointer, when a tag is what is being carried. */
  overRow: number | null;
};

/**
 * Dragging conversations with the pointer.
 *
 * The target under the pointer is found by asking the document what is at that
 * point rather than by each destination listening for its own events. That is
 * the whole reason this is reliable: there is one place that decides what is
 * being hovered, it works the same in every engine, and a destination cannot
 * miss a drag that passed over it because a child element swallowed the event.
 */
export function useDrag(
  currentView: string,
  onDrop: (railKey: string, ids: number[]) => void,
  onTagRow: (tagId: number, threadId: number) => void,
) {
  const [drag, setDrag] = useState<Dragging | null>(null);
  // Held in a ref as well: the window listeners below are registered once and
  // would otherwise close over the first render's value forever.
  const live = useRef<Dragging | null>(null);
  const pending = useRef<{ payload: Payload; label: string; x: number; y: number } | null>(null);

  const set = useCallback((next: Dragging | null) => {
    live.current = next;
    setDrag(next);
    // The grabbing hand belongs to the window, not the row: the pointer spends
    // the drag over the rail and the reader, each of which has a cursor of its
    // own that would otherwise take over.
    document.body.classList.toggle('dragging', next !== null);
  }, []);

  /**
   * What the pointer is over, if it is something that takes what is being
   * carried. Conversations look for a rail destination; a tag looks for a
   * conversation, and the two never answer for each other.
   */
  const targetAt = useCallback(
    (payload: Payload, x: number, y: number): { over: string | null; overRow: number | null } => {
      const el = document.elementFromPoint(x, y);
      if (payload.kind === 'tag') {
        const row = el?.closest<HTMLElement>('[data-drop-row]');
        const id = Number(row?.dataset.dropRow);
        return { over: null, overRow: Number.isFinite(id) && id !== 0 ? id : null };
      }
      const host = el?.closest<HTMLElement>('[data-drop-key]');
      const key = host?.dataset.dropKey;
      return { over: key && acceptsDrop(key, currentView) ? key : null, overRow: null };
    },
    [currentView],
  );

  useEffect(() => {
    function move(e: PointerEvent) {
      const start = pending.current;
      if (start && !live.current) {
        // Still deciding whether this is a click or a drag.
        if (Math.hypot(e.clientX - start.x, e.clientY - start.y) < DRAG_THRESHOLD) return;
        set({
          ...start,
          x: e.clientX,
          y: e.clientY,
          ...targetAt(start.payload, e.clientX, e.clientY),
        });
        return;
      }
      if (!live.current) return;
      e.preventDefault();
      set({
        ...live.current,
        x: e.clientX,
        y: e.clientY,
        ...targetAt(live.current.payload, e.clientX, e.clientY),
      });
    }

    function up(e: PointerEvent) {
      const held = live.current;
      pending.current = null;
      if (!held) return;
      set(null);
      const hit = targetAt(held.payload, e.clientX, e.clientY);
      if (held.payload.kind === 'tag') {
        if (hit.overRow !== null) onTagRow(held.payload.tagId, hit.overRow);
      } else if (hit.over) {
        onDrop(hit.over, held.payload.ids);
      }
    }

    // Escape abandons the drag without dropping. A gesture you have committed
    // to but changed your mind about needs a way out that is not "drop it
    // somewhere harmless and undo".
    function key(e: KeyboardEvent) {
      if (e.key === 'Escape' && live.current) {
        pending.current = null;
        set(null);
      }
    }

    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
    window.addEventListener('pointercancel', up);
    window.addEventListener('keydown', key);
    return () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      window.removeEventListener('pointercancel', up);
      window.removeEventListener('keydown', key);
    };
  }, [onDrop, onTagRow, set, targetAt]);

  /** Attach to a row: begins a drag once the pointer has travelled far enough. */
  const start = useCallback(
    (e: React.PointerEvent, rowId: number, selected: ReadonlySet<number>, subject: string) => {
      if (e.button !== 0) return;
      // Nothing to drag to, so nothing to drag. Starting a drag that no
      // destination would accept is a gesture that can only end in nothing
      // happening, which reads as the feature being broken.
      if (!draggableFrom(currentView)) return;
      // Controls *inside* the row are things you press, not handles you pull —
      // the checkbox, the row menu. Compared against the row itself because the
      // row is a button too, so the nearest control to anything in it is the
      // row, and a bare `closest('button')` rejected every drag there was.
      const control = (e.target as HTMLElement).closest('button, [role="button"], input, a');
      if (control && control !== e.currentTarget) return;
      const ids = draggedIds(rowId, selected);
      pending.current = {
        payload: { kind: 'threads', ids },
        label: ids.length > 1 ? '' : subject,
        x: e.clientX,
        y: e.clientY,
      };
    },
    [currentView],
  );

  /**
   * Attach to a tag in the rail: begins carrying that tag to a conversation.
   *
   * The same gesture read the other way round. Dragging a tag onto a message is
   * how you file the one thing in front of you; dragging messages onto a tag is
   * how you file several at once. Neither is the "real" direction.
   */
  const startTag = useCallback((e: React.PointerEvent, tagId: number, name: string) => {
    if (e.button !== 0) return;
    // The tag's own edit menu is a control on the row, not a handle.
    const control = (e.target as HTMLElement).closest('button, [role="button"]');
    if (control && control !== e.currentTarget) return;
    pending.current = {
      payload: { kind: 'tag', tagId, name },
      label: name,
      x: e.clientX,
      y: e.clientY,
    };
  }, []);

  return { drag, start, startTag };
}
