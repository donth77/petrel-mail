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
 * Why this is not HTML5 drag and drop.
 *
 * It was, and it did not work in the app. WebKit refuses to begin a drag on an
 * element whose computed `user-select` is `none`, and a conversation row is a
 * button, which the user-agent stylesheet makes `user-select: none`. Chromium
 * has no such rule, so the browser harness dragged happily while the real
 * application did nothing at all — no drag image, no drop targets, no drop.
 *
 * Pointer events have no such divergence, and the drag image stops being
 * whatever the engine decides to photograph: it is an element, so it can say
 * what is being carried. The rail's resize handle already works this way, so
 * this is the pattern the app already had rather than a new one.
 *
 * The cost is that these drags exist only inside the window — nothing can be
 * dragged out to the Finder. That was never possible with the previous
 * approach either, since a conversation is not a file.
 */

/** How far the pointer must travel before a press becomes a drag, in pixels.
 *  Below this a click is a click: a list you cannot click without dragging is
 *  far worse than one you cannot drag. */
export const DRAG_THRESHOLD = 5;

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
 * Whether conversations in this mailbox can be dragged at all.
 *
 * The Outbox is the exception: those messages are mid-flight, waiting on a
 * timer or a connection, and the only useful things to do with one are edit it
 * or call it back. Filing a message that is in the act of leaving is not a
 * request the app can honour, so the rows do not offer it.
 */
export function draggableFrom(view: string): boolean {
  return view !== 'outbox';
}

/**
 * Whether a rail destination will accept conversations dragged from this view.
 *
 * Dropping mail where it already is achieves nothing, so the view you are
 * looking at declines the drag rather than lighting up and doing nothing.
 *
 * Mail you wrote is the other restriction. Inbox, Archive and Spam are stations
 * in the life of something that arrived: a sent message was never in the inbox,
 * so "archive" has nothing to mean, and marking your own message as spam is a
 * statement about yourself the server will not act on. Throwing it away and
 * marking it — Trash, Starred, a tag — apply to anything at all, so those stay.
 */
export function acceptsDrop(railKey: string, currentView: string): boolean {
  if (railKey === currentView) return false;
  if (!draggableFrom(currentView)) return false;
  const meaning = dropMeaning(railKey);
  if (meaning === null) return false;
  if (currentView === 'sent' || currentView === 'drafts') {
    return meaning.kind === 'trash' || meaning.kind === 'star' || meaning.kind === 'tag';
  }
  return true;
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
