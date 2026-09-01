/** Conversations asked for in one IPC call.
 *
 *  The engine pages by walking an index until the page is full. A hundred
 *  fills a tall window in one round-trip; the store lock is held for the
 *  walk plus the row fetch, both of which stay well under the list-open
 *  budget at this size. The IPC cap matches. */
export const LIST_PAGE = 100;

/** How close to the loaded tail the virtual window must be to ask for more. */
export const LIST_NEAR_END = 12;

/** Compact rows are one line; the virtualizer never measures them. */
export const COMPACT_ROW = 30;

/** Relaxed rows without chips. Padding plus three lines at the list's
 *  line-height. */
export const RELAXED_ROW = 74;

/** Relaxed rows with an attachment or tag chip. The extra band is why
 *  scrolling up used to hitch: first-measure then rewrote scrollTop. */
export const CHIP_ROW = 94;

/** Whether the list should request the next page.
 *
 *  Two pages without a scroll fill a tall window. Past that, a viewport that
 *  grew with the spacer (virtualization never clipped) would otherwise walk
 *  the whole mailbox into React because every row looks visible. */
export function shouldRequestNextPage(
  lastVisibleIndex: number,
  itemCount: number,
  scrollTop: number,
): boolean {
  if (itemCount === 0) return false;
  if (lastVisibleIndex < itemCount - 1 - LIST_NEAR_END) return false;
  if (scrollTop <= 0 && itemCount >= LIST_PAGE * 2) return false;
  return true;
}

/** Extra scrollTop after new conversations land at the head.
 *
 *  Prepend without this leaves the same pixel offset showing different
 *  rows. At the top of the list the new mail should appear above, so a
 *  zero (or negative) offset is left alone. A full replace does not keep
 *  the previous head, so it also returns zero. */
export function prependScrollDelta(args: {
  previousHeadId: number | undefined;
  items: readonly { id: number }[];
  scrollTop: number;
  rowHeight: number;
}): number {
  if (args.scrollTop <= 0 || args.previousHeadId == null || args.rowHeight <= 0) {
    return 0;
  }
  const at = args.items.findIndex((m) => m.id === args.previousHeadId);
  if (at <= 0) return 0;
  return at * args.rowHeight;
}
