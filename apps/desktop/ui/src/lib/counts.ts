import type { ActionKind } from './api';
import { defaultCount } from './mailboxes';

/** What the rail's numbers mean, from the Badges setting. */
export type CountMode = 'unread' | 'total' | 'off';

/**
 * The mailboxes `view_counts` reports on. Anything else — a tag view, a user
 * folder — has no number in this map, so a conversation arriving in or leaving
 * one changes nothing here.
 */
const COUNTED = new Set([
  'inbox',
  'starred',
  'snoozed',
  'archive',
  'spam',
  'trash',
  'drafts',
  'outbox',
  'sent',
]);

/**
 * Whether this conversation is one of the ones a given number is counting.
 *
 * Mirrors `Store::view_counts`, which now answers per mailbox: what each row
 * counts lives in one place, `mailboxes.ts`, beside the engine's own rule. So
 * this asks only the question that is left once the row's mode is known — does
 * a number that counts *this* mode move for *this* conversation.
 */
function counted(mode: CountMode, unread: boolean): boolean {
  if (mode === 'off') return false;
  return mode === 'total' || unread;
}

/** The counted mailbox an action puts the conversation into, if any. */
function destination(kind: ActionKind, toRole?: string): string | null {
  if (kind === 'trash') return 'trash';
  if (kind === 'spam') return 'spam';
  if (kind === 'archive') return 'archive';
  if (kind === 'snooze') return 'snoozed';
  // A move names a folder. Only the inbox has a number here; filing into a
  // folder of your own changes no badge, because folders carry none.
  if (kind === 'move') return toRole === 'inbox' ? 'inbox' : null;
  // Deleting for good, unstarring, unsnoozing: the conversation leaves
  // somewhere and arrives nowhere that is counted. Unsnooze is deliberately
  // not given the inbox: where an unsnoozed conversation lands is the engine's
  // decision, and guessing it here would be inventing a number.
  return null;
}

/**
 * How the rail's numbers should move for a triage action, applied before the
 * engine has been asked.
 *
 * One rule: the conversation left the view it was in, so that number drops; it
 * landed in a counted mailbox, so that number rises. Both halves are gated on
 * whether this particular conversation is one the number counts at all.
 *
 * Deliberately best-effort. A row carries no placement, so the only source
 * this can name is the view you are looking at — trashing from a tag view
 * knows the bin gained one but not which mailbox lost one. That is why the
 * debounced recount after every triage stays the authority: this decides what
 * the number says for the next 300ms, not what is true.
 */
export function countDeltas(opts: {
  kind: ActionKind;
  /** The view the conversation was listed in. */
  view: string;
  /** Whether the conversation counts as unread — the usual mode's question. */
  unread: boolean;
  /** Whether the action takes the row out of that view (`leavesView`). */
  removes: boolean;
  /** What each mailbox counts, from the sidebar arrangement. A row this does
   *  not name falls to its default, the same as in the engine. */
  modes: Record<string, CountMode>;
  /** The role of the folder a `move` names, when the folder has one. */
  toRole?: string;
}): Record<string, number> {
  const { kind, view, unread, removes, modes, toRole } = opts;
  const modeOf = (key: string) => modes[key] ?? defaultCount(key);
  const out: Record<string, number> = {};

  const to = destination(kind, toRole);
  // `to !== view` is what stops trashing from the bin, or archiving from the
  // archive, claiming the conversation arrived somewhere it already was.
  if (to && to !== view && counted(modeOf(to), unread)) out[to] = 1;

  if (removes && COUNTED.has(view) && counted(modeOf(view), unread)) {
    out[view] = (out[view] ?? 0) - 1;
  }
  return out;
}
