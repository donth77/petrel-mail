import type { StringId } from './strings';

/**
 * What a conversation list is ordered by.
 *
 * One vocabulary for the mailbox and for search results, which is the point:
 * before this, a search had two buttons of its own and a mailbox had no
 * control at all, so "how is this list ordered" had two different answers
 * depending on whether the box above it had anything in it.
 *
 * `relevance` is the one a mailbox cannot offer, because relevance is to a
 * query and a mailbox has none. It is not a fourth key so much as the absence
 * of one: leave the ranking as the search found it.
 */
export type SortKey = 'relevance' | 'date' | 'sender' | 'subject';

export type Sort = { key: SortKey; ascending: boolean };

/** Newest first, because that is what a mailbox is for. */
export const DEFAULT_SORT: Sort = { key: 'date', ascending: false };

/** A search opens on its ranking; that is what searching is for. */
export const SEARCH_SORT: Sort = { key: 'relevance', ascending: false };

/** The keys on offer. `relevance` only where there is a query to rank against. */
export function sortKeys(searching: boolean): SortKey[] {
  return searching ? ['relevance', 'date', 'sender', 'subject'] : ['date', 'sender', 'subject'];
}

export const KEY_LABEL: Record<SortKey, StringId> = {
  relevance: 'sort-relevance',
  date: 'sort-date',
  sender: 'sort-sender',
  subject: 'sort-subject',
};

/**
 * What the two directions are called for a key.
 *
 * "Ascending" is a word about numbers, and a list of names sorted ascending is
 * a list sorted A to Z. Saying which is which in the key's own terms is the
 * difference between a control somebody reads and one they experiment with.
 */
export function directionLabels(key: SortKey): { ascending: StringId; descending: StringId } {
  return key === 'date'
    ? { ascending: 'sort-oldest', descending: 'sort-newest' }
    : { ascending: 'sort-a-z', descending: 'sort-z-a' };
}

/**
 * The sort actually applied, given whether a search is running.
 *
 * Relevance survives only while there is a query. Leaving the list on it after
 * the box empties would leave a mailbox claiming an order it cannot have, so
 * it falls back to the default rather than to nothing.
 */
export function effectiveSort(sort: Sort, searching: boolean): Sort {
  if (sort.key === 'relevance' && !searching) return DEFAULT_SORT;
  return sort;
}

/** What the engine is told. `relevance` is the absence of a sort, not a key. */
export function wireSort(sort: Sort): { key: string | undefined; ascending: boolean } {
  return {
    key: sort.key === 'relevance' ? undefined : sort.key,
    ascending: sort.ascending,
  };
}

/** A sort as a setting holds it: `key:direction`. */
export function writeSort(sort: Sort): string {
  return `${sort.key}:${sort.ascending ? 'ascending' : 'descending'}`;
}

/** A sort read back from a setting, or the fallback if it says nothing
 *  this version knows: a stored preference must never be a broken list. */
export function readSort(said: string, fallback: Sort): Sort {
  const [key, direction] = said.split(':');
  const known: SortKey[] = ['relevance', 'date', 'sender', 'subject'];
  if (!known.includes(key as SortKey)) return fallback;
  return { key: key as SortKey, ascending: direction === 'ascending' };
}

/**
 * Which mailbox is ordered how, as a setting holds it.
 *
 * A mailbox remembers its own order, the way it does in Mail, Outlook,
 * Thunderbird and the Finder: Sent is a list of people you wrote to and
 * reads well by name, while the Inbox almost never does. What a view has not
 * been given falls back to the one order the window keeps, so a fresh
 * install is consistent and only what somebody changed is different.
 */
export type SortByView = Record<string, string>;

export function readSortByView(said: string): SortByView {
  try {
    const parsed: unknown = JSON.parse(said || '{}');
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) return {};
    // Values only, and only strings: a setting is text somebody could edit.
    return Object.fromEntries(
      Object.entries(parsed as Record<string, unknown>).filter(
        ([, said]) => typeof said === 'string',
      ) as [string, string][],
    );
  } catch {
    return {};
  }
}

/** The order a view is in: its own, or the one the window keeps. */
export function sortForView(by: SortByView, view: string, fallback: Sort): Sort {
  const said = by[view];
  return said === undefined ? fallback : readSort(said, fallback);
}

/** That view, ordered this way, with the rest as they were. */
export function withViewSort(by: SortByView, view: string, sort: Sort): string {
  return JSON.stringify({ ...by, [view]: writeSort(sort) });
}
