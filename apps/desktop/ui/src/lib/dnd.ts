import type { ActionKind } from './api';

/**
 * Dragging conversations onto the rail.
 *
 * The gesture is a shortcut, never the only way: everything here is also a
 * keystroke and a menu item, because a drag is hard to undo halfway, impossible
 * from the keyboard, and unavailable to anyone who cannot hold a button down
 * while moving a pointer accurately.
 *
 * What a drop *means* is decided here rather than at the drop site, so the rail
 * can ask "does this accept a drop?" while painting and "what did that mean?"
 * on release and get answers that cannot disagree.
 */

/**
 * The drag's payload type.
 *
 * A private MIME type rather than `text/plain`: it keeps conversation ids from
 * being dropped into a text field somewhere as a row of numbers, and it lets a
 * drop target tell our drag from a file being dragged in from the desktop.
 */
export const DRAG_TYPE = 'application/x-petrel-threads';

/** What dropping on a rail destination does, or null if it takes no drops. */
export type DropMeaning =
  | { kind: Extract<ActionKind, 'archive' | 'trash' | 'spam' | 'star'> }
  | { kind: 'tag'; tag: string }
  | { kind: 'move'; role: 'inbox' };

/**
 * Reads a rail key as a destination.
 *
 * Deliberately a short list. Sent, Drafts and the Outbox describe how a message
 * came to exist rather than where it is filed, and dropping mail into them
 * would be claiming you sent it. Snoozed needs a time before it means anything,
 * and a drop that opens a dialog is a drop that went wrong. Those show no
 * response to a drag at all, which is a clearer answer than accepting one and
 * doing nothing.
 */
export function dropMeaning(railKey: string): DropMeaning | null {
  if (railKey.startsWith('tag:')) {
    const tag = railKey.slice('tag:'.length);
    return tag ? { kind: 'tag', tag } : null;
  }
  switch (railKey) {
    case 'archive':
      return { kind: 'archive' };
    case 'trash':
      return { kind: 'trash' };
    case 'spam':
      return { kind: 'spam' };
    case 'starred':
      return { kind: 'star' };
    // The way back out of Archive, and the reason Inbox is a destination at all.
    case 'inbox':
      return { kind: 'move', role: 'inbox' };
    default:
      return null;
  }
}

/**
 * Whether a rail destination will accept the conversations being dragged.
 *
 * Dropping mail where it already is achieves nothing, so the view you are
 * looking at declines the drag rather than lighting up and doing nothing.
 */
export function acceptsDrop(railKey: string, currentView: string): boolean {
  if (railKey === currentView) return false;
  return dropMeaning(railKey) !== null;
}

/**
 * Which conversations a drag carries.
 *
 * Dragging a row inside the selection takes the whole selection; dragging one
 * outside it takes only that row, and does not quietly discard the selection —
 * the same rule every file manager uses, and the one people already expect.
 */
export function draggedIds(rowId: number, selected: ReadonlySet<number>): number[] {
  return selected.has(rowId) ? [...selected] : [rowId];
}
