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
): Set<number> {
  const from = anchorId == null ? -1 : order.indexOf(anchorId);
  const to = order.indexOf(id);
  if (to < 0) return new Set(selected);
  if (from < 0) return new Set([id]);
  const [lo, hi] = from <= to ? [from, to] : [to, from];
  return new Set(order.slice(lo, hi + 1));
}

/** Drops ids no longer in the list, so a selection cannot outlive its rows. */
export function prune(selected: ReadonlySet<number>, order: readonly number[]): Set<number> {
  const present = new Set(order);
  const next = new Set<number>();
  for (const id of selected) if (present.has(id)) next.add(id);
  return next;
}
