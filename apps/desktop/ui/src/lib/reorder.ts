/**
 * Putting a dragged row's new order back into the whole list.
 *
 * A drag can only rearrange what is on screen, and the sidebar rarely shows
 * everything: subtrees are folded, Archive and Trash hang off their own rows,
 * and a flyout card draws outside the rail entirely. So the order read off the
 * rendered rows is a *subset* of the account's folders — and saving it as if it
 * were the whole list is what corrupted the stored order.
 *
 * The engine numbers the ids it is given from zero. Hand it nine of eleven
 * folders and those nine take 0..8 while the other two keep numbers they were
 * given by some earlier drag — so two folders claim 0, another two claim 1, and
 * on the next launch the list comes back interleaved. A live account had three
 * folders at position 0, two at 1, two at 4 and nothing at 5.
 *
 * This makes the saved order total again: the rows that were on screen land in
 * the order they were dragged into, and every row that was not keeps the slot
 * it already held.
 */
export function mergeOrder(full: number[], reordered: number[]): number[] {
  const present = new Set(full);
  // Only ids the list actually holds. A row that vanished mid-drag — the sync
  // pruned it, another window deleted it — would otherwise consume a slot and
  // leave the tail undefined.
  const queue = reordered.filter((id) => present.has(id));
  const moving = new Set(queue);
  let next = 0;
  return full.map((id) => (moving.has(id) ? queue[next++] : id));
}
