import type { StringId } from './strings';

/**
 * Which groups the rail draws, and in what order.
 *
 * The rail used to hold its sections in the order they were written: mailboxes,
 * folders, tags, and later searches. Whose mail looks like that is a guess —
 * somebody who lives in folders wants them first, somebody who tags everything
 * wants tags, and somebody with three pinned searches wants those. The order is
 * theirs to set, and so is whether a section appears at all.
 *
 * Mailboxes is the exception on both counts of hiding: it holds the Inbox, and a
 * rail with no way to reach your mail is not a rail. It can be moved, because
 * moving it hides nothing.
 *
 * Stored as JSON in one setting, the way `railMailboxes` stores the arrangement
 * inside the mailboxes section. Read tolerantly: a section this version does not
 * know is dropped, one it knows and the stored order omits is appended in
 * shipped order, so a store written by another version is never a rail with a
 * group missing.
 */
export type SectionKey = 'mailboxes' | 'folders' | 'tags' | 'searches';

/** Shipped order. Places, then labels, then questions — and the questions last
 *  because a section nobody has filled should not push the rest down. */
export const SECTIONS: SectionKey[] = ['mailboxes', 'folders', 'tags', 'searches'];

/** The one that cannot be hidden. */
export const ESSENTIAL: SectionKey = 'mailboxes';

export const SECTION_LABEL: Record<SectionKey, StringId> = {
  mailboxes: 'rail-mailboxes',
  folders: 'rail-folders',
  tags: 'rail-tags',
  searches: 'rail-searches',
};

export type Sections = { order: SectionKey[]; hidden: SectionKey[] };

const isKey = (value: unknown): value is SectionKey =>
  typeof value === 'string' && (SECTIONS as string[]).includes(value);

export function shippedSections(): Sections {
  return { order: [...SECTIONS], hidden: [] };
}

export function readSections(said: string): Sections {
  let parsed: unknown;
  try {
    parsed = JSON.parse(said || '{}');
  } catch {
    return shippedSections();
  }
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) return shippedSections();
  const from = parsed as { order?: unknown; hidden?: unknown };
  const stored = Array.isArray(from.order) ? from.order.filter(isKey) : [];
  // Anything the stored order left out goes back where it shipped, so a
  // section added by a later version is not lost by an earlier one saving over
  // it — and so a corrupt setting still draws a whole rail.
  const order = [...new Set([...stored, ...SECTIONS])];
  const hidden = (Array.isArray(from.hidden) ? from.hidden.filter(isKey) : []).filter(
    (key) => key !== ESSENTIAL,
  );
  return { order, hidden: [...new Set(hidden)] };
}

export function writeSections(s: Sections): string {
  return JSON.stringify({
    order: s.order,
    hidden: s.hidden.filter((key) => key !== ESSENTIAL),
  });
}

/** The sections to draw, in order. */
export function visibleSections(s: Sections): SectionKey[] {
  return s.order.filter((key) => key === ESSENTIAL || !s.hidden.includes(key));
}

export function isHidden(s: Sections, key: SectionKey): boolean {
  return key !== ESSENTIAL && s.hidden.includes(key);
}

/** One section shown or hidden. Mailboxes cannot be hidden, so asking does
 *  nothing rather than erroring — the switch for it is not rendered anyway. */
export function toggledSection(s: Sections, key: SectionKey): Sections {
  if (key === ESSENTIAL) return s;
  return isHidden(s, key)
    ? { ...s, hidden: s.hidden.filter((k) => k !== key) }
    : { ...s, hidden: [...s.hidden, key] };
}

/**
 * One section a step up or down.
 *
 * A swap with its neighbour, which is the whole of reordering here: four rows
 * with two buttons each is a control anyone can use, and it needs no pointer.
 * At either end it does nothing, so the button can be present and inert rather
 * than appearing and disappearing as a row travels.
 */
export function movedSection(s: Sections, key: SectionKey, up: boolean): Sections {
  const at = s.order.indexOf(key);
  const to = up ? at - 1 : at + 1;
  if (at < 0 || to < 0 || to >= s.order.length) return s;
  const order = [...s.order];
  [order[at], order[to]] = [order[to], order[at]];
  return { ...s, order };
}

/**
 * Which section a view belongs to.
 *
 * Needed because a hidden section can still be the view you are standing in:
 * hide Tags while looking at `tag:Urgent` and the list stays, the rail has no
 * row for it, and nothing is current — the window showing a collection it has
 * just removed the way to reach. The caller walks back to the inbox.
 *
 * Everything the grammar cannot place is a mailbox, which is where the fixed
 * rows live and where a fallback belongs.
 */
export function sectionOf(view: string): SectionKey {
  if (view.startsWith('tag:')) return 'tags';
  if (view.startsWith('folder:')) return 'folders';
  if (view.startsWith('search:')) return 'searches';
  return 'mailboxes';
}
