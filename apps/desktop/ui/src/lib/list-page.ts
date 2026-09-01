/** Conversations asked for in one IPC call.
 *
 *  The engine pages by walking an index until the page is full. A hundred
 *  fills a tall window in one round-trip; the store lock is held for the
 *  walk plus the row fetch, both of which stay well under the list-open
 *  budget at this size. The IPC cap matches. */
export const LIST_PAGE = 100;

/** How close to the loaded tail the virtual window must be to ask for more. */
export const LIST_NEAR_END = 12;

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
