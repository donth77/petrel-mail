/**
 * Which conversations an action applies to.
 *
 * The rule everywhere: if anything is selected, actions act on the selection;
 * otherwise they act on whatever is highlighted. That is what makes X worth
 * having — every key you already know keeps working, on more than one thing.
 *
 * Kept out of the components because three of them need the same answer, and
 * three copies of "which ones did they mean" is how bulk archive ends up
 * disagreeing with bulk star about what it archived.
 */

/** The ids an action should apply to. */
export function targets(selected: ReadonlySet<number>, activeId: number | null): number[] {
  if (selected.size > 0) return [...selected];
  return activeId == null ? [] : [activeId];
}

/** Toggles one id, returning a new set. */
export function toggle(selected: ReadonlySet<number>, id: number): Set<number> {
  const next = new Set(selected);
  if (!next.delete(id)) next.add(id);
  return next;
}

/**
 * Extends a selection from the anchor to `id`, inclusive, in list order.
 *
 * Range rather than "add this one": ⇧J down a list should select what it
 * passed over, and a shift-click should reach back to where you started. The
 * anchor is where the range grows from, so reversing direction shrinks it
 * again instead of leaving a trail nobody meant to select.
 */
export function extend(
  selected: ReadonlySet<number>,
  order: readonly number[],
  anchorId: number | null,
  id: number,
  /** Where the last range from this anchor ended. That range is redrawn;
   *  rows picked before it, or outside it, stay picked. Without this a
   *  ⇧-click replaced the whole selection with the new range, and a row
   *  checked earlier was quietly dropped. */
  prevEndId: number | null = null,
): Set<number> {
  const from = anchorId == null ? -1 : order.indexOf(anchorId);
  const to = order.indexOf(id);
  if (to < 0) return new Set(selected);
  if (from < 0) return new Set([id]);
  const next = new Set(selected);
  const prev = prevEndId == null ? -1 : order.indexOf(prevEndId);
  if (prev >= 0) {
    const [plo, phi] = from <= prev ? [from, prev] : [prev, from];
    for (const rid of order.slice(plo, phi + 1)) next.delete(rid);
  }
  const [lo, hi] = from <= to ? [from, to] : [to, from];
  for (const rid of order.slice(lo, hi + 1)) next.add(rid);
  return next;
}

/** Drops ids no longer in the list, so a selection cannot outlive its rows. */
export function prune(selected: ReadonlySet<number>, order: readonly number[]): Set<number> {
  const present = new Set(order);
  const next = new Set<number>();
  for (const id of selected) if (present.has(id)) next.add(id);
  return next;
}

/**
 * How a group of conversations reads and shines, as one thing.
 *
 * For the two toggles that name a direction. A menu opened on a selection
 * has to take that direction from the whole selection: read off the row
 * under the pointer, a mixed selection right-clicked on a read row offered
 * "Mark as unread" and turned every row unread, the unread ones included.
 *
 * The rule is the one Gmail and Apple Mail use. Any unread conversation makes
 * the group unread, so the offer is Mark as read and it reaches every row that
 * needs it; only a group that is entirely read is offered Mark as unread.
 * Star is the same shape: unstarred until every row wears one.
 */
export function facing(
  rows: readonly { unread: boolean; starred: boolean }[],
): { unread: boolean; starred: boolean } {
  return {
    unread: rows.some((r) => r.unread),
    starred: rows.length > 0 && rows.every((r) => r.starred),
  };
}

/**
 * The rows a list of ids names, skipping any the list no longer holds.
 *
 * Matched the way `run` matches, by row id or by conversation id, so whoever
 * asks agrees with the action about which conversations are in play.
 */
export function rowsOf<T extends { id: number; thread_id: number }>(
  ids: readonly number[],
  items: readonly T[],
): T[] {
  return ids.flatMap((id) => {
    const m = items.find((r) => r.id === id || r.thread_id === id);
    return m ? [m] : [];
  });
}

/**
 * The tags every one of these conversations carries.
 *
 * A tag on some of them and not others is not "on" for the group. Reading it
 * as on, the picker offered to take it off, and did: the rows that had it
 * lost it and the rows that did not gained nothing. Read as off, the offer is
 * to apply it, which reaches the rows that need it and leaves the rest as
 * they were — the rule Gmail and Apple Mail use, and the same shape as
 * `facing` for read state and stars.
 */
export function tagsOnAll(rows: readonly { tags: readonly { name: string }[] }[]): Set<string> {
  const [first, ...rest] = rows;
  if (!first) return new Set();
  const on = new Set(first.tags.map((x) => x.name));
  for (const r of rest) {
    const names = new Set(r.tags.map((x) => x.name));
    for (const n of [...on]) if (!names.has(n)) on.delete(n);
  }
  return on;
}
