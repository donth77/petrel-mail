import { useCallback, useEffect, useRef, useState } from 'react';
import type { Thread } from './api';
import { LIST_PAGE } from './list-page';
import { wireSort, type Sort } from './sort';

export type ThreadFetchers = {
  threads: (
    view: string,
    offset: number,
    limit: number,
    sort?: string,
    ascending?: boolean,
    beforeDateMs?: number,
    beforeThreadId?: number,
  ) => Promise<Thread[]>;
  search: (query: string, sort?: string, ascending?: boolean) => Promise<Thread[]>;
};

/** First listing page — offset zero, no keyset cursor. */
export function firstPageCall(view: string, sort: Sort): Parameters<ThreadFetchers['threads']> {
  const wire = wireSort(sort);
  return [view, 0, LIST_PAGE, wire.key, wire.ascending];
}

/** Next listing page — cursor taken from the last loaded row. */
export function loadMoreCall(view: string, sort: Sort, last: Thread): Parameters<ThreadFetchers['threads']> {
  const wire = wireSort(sort);
  return [view, 0, LIST_PAGE, wire.key, wire.ascending, last.date_ms, last.thread_id];
}

export function replaceLoadHasMore(query: string, rowCount: number): boolean {
  return !query.trim() && rowCount === LIST_PAGE;
}

/** The mailbox changed under a loaded window: fold in a fresh first page.
 *
 *  The fresh page is the truth for everything it covers, so it goes first in
 *  its own order — a conversation that just gained a reply moves to the top
 *  rather than sitting where it was with a new dot on it — and the rows it
 *  covers leave their old places in the tail. A short page is the whole view,
 *  so the tail goes with it. A full page, sorted by date, also says which
 *  tail rows have gone: anything newer than its last row that it does not
 *  list was deleted or filed elsewhere by another client. Other sorts have no
 *  such range, so their tails are kept as they were. */
export function mergeHead(
  prev: Thread[],
  incoming: Thread[],
  order: { byDate: boolean; ascending: boolean } = { byDate: false, ascending: false },
): Thread[] {
  if (incoming.length < LIST_PAGE) return incoming;
  const covered = new Set(incoming.map((t) => t.thread_id));
  const edge = incoming[incoming.length - 1].date_ms;
  const insidePage = (t: Thread) =>
    order.byDate && (order.ascending ? t.date_ms < edge : t.date_ms > edge);
  const rest = prev.filter((t) => !covered.has(t.thread_id) && !insidePage(t));
  return [...incoming, ...rest];
}

export function appendPage(
  prev: Thread[],
  incoming: Thread[],
): { items: Thread[]; reachedEnd: boolean } {
  const seen = new Set(prev.map((t) => t.thread_id));
  const append = incoming.filter((t) => !seen.has(t.thread_id));
  return { items: [...prev, ...append], reachedEnd: incoming.length < LIST_PAGE };
}

export async function runReplaceLoad(
  fetchers: ThreadFetchers,
  query: string,
  view: string,
  sort: Sort,
): Promise<{ items: Thread[]; hasMore: boolean }> {
  const trimmed = query.trim();
  const wire = wireSort(sort);
  if (trimmed) {
    const items = await fetchers.search(trimmed, wire.key, wire.ascending);
    return { items, hasMore: false };
  }
  const items = await fetchers.threads(...firstPageCall(view, sort));
  return { items, hasMore: replaceLoadHasMore(query, items.length) };
}

export function useThreadWindow(args: {
  query: string;
  view: string;
  sort: Sort;
  accountEpoch: number;
  /** Live message count. Increases mean new mail — merge into the head, never replace the loaded window. */
  messageCount: number | undefined;
  fetchers: ThreadFetchers;
}): {
  items: Thread[];
  setItems: React.Dispatch<React.SetStateAction<Thread[]>>;
  loading: boolean;
  error: string | null;
  hasMore: boolean;
  loadMore: () => void;
  /** Bumps when the loaded window is replaced (view, query, sort, account)
   *  or when the first mail lands in an empty list. Paging and new mail at
   *  the head do not bump it — the highlight must not jump just because
   *  the array is new. */
  replaceEpoch: number;
} {
  const { query, view, sort, accountEpoch, messageCount, fetchers } = args;

  const [items, setItems] = useState<Thread[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [hasMore, setHasMore] = useState(false);

  const itemsRef = useRef(items);
  itemsRef.current = items;

  const queryRef = useRef(query);
  queryRef.current = query;

  const hasMoreRef = useRef(hasMore);
  hasMoreRef.current = hasMore;

  const fetchersRef = useRef(fetchers);
  fetchersRef.current = fetchers;

  const viewRef = useRef(view);
  viewRef.current = view;

  const sortRef = useRef(sort);
  sortRef.current = sort;

  const loadMoreInFlight = useRef(false);
  const messageCountRef = useRef(messageCount);
  const [replaceEpoch, setReplaceEpoch] = useState(0);

  // Replace the window when the mailbox, query, sort, or account changes.
  useEffect(() => {
    let live = true;
    setLoading(true);

    const debounceMs = query.trim() ? 100 : 0;
    const handle = window.setTimeout(() => {
      runReplaceLoad(fetchersRef.current, query, view, sort)
        .then(({ items: rows, hasMore: more }) => {
          if (!live) return;
          setError(null);
          setItems(rows);
          setHasMore(more);
          setReplaceEpoch((n) => n + 1);
          setLoading(false);
        })
        .catch((err: unknown) => {
          if (!live) return;
          setError(String(err));
          setLoading(false);
        });
    }, debounceMs);

    return () => {
      live = false;
      window.clearTimeout(handle);
    };
  }, [query, view, sort, accountEpoch]);

  // On a reset, remember the count without treating it as new mail.
  useEffect(() => {
    messageCountRef.current = messageCount;
    // `messageCount` is the snapshot we store, not a trigger. Including it
    // would collapse "new mail" into "the mailbox changed" and skip the merge.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [query, view, sort, accountEpoch]);

  // A changed count with no search running: fold a fresh first page into
  // the window. Up is new mail; down is something deleted elsewhere, and the
  // row it left behind should go the same way.
  useEffect(() => {
    if (messageCount === undefined || query.trim()) return;

    const prev = messageCountRef.current;
    messageCountRef.current = messageCount;
    if (prev === undefined || messageCount === prev) return;

    let live = true;
    fetchersRef.current
      .threads(...firstPageCall(viewRef.current, sortRef.current))
      .then((rows) => {
        if (!live) return;
        const wasEmpty = itemsRef.current.length === 0;
        const sort = sortRef.current;
        setItems((cur) =>
          mergeHead(cur, rows, { byDate: sort.key === 'date', ascending: sort.ascending }),
        );
        if (wasEmpty) {
          setHasMore(rows.length === LIST_PAGE);
          setReplaceEpoch((n) => n + 1);
        }
      })
      .catch((err: unknown) => {
        if (!live) return;
        setError(String(err));
      });

    return () => {
      live = false;
    };
  }, [messageCount, query]);

  const loadMore = useCallback(() => {
    if (queryRef.current.trim() || !hasMoreRef.current || loadMoreInFlight.current) return;

    const current = itemsRef.current;
    const last = current[current.length - 1];
    if (!last) return;

    loadMoreInFlight.current = true;
    const wire = wireSort(sortRef.current);
    fetchersRef.current
      .threads(
        viewRef.current,
        0,
        LIST_PAGE,
        wire.key,
        wire.ascending,
        last.date_ms,
        last.thread_id,
      )
      .then((rows) => {
        const { items: next, reachedEnd } = appendPage(itemsRef.current, rows);
        setItems(next);
        if (reachedEnd) setHasMore(false);
      })
      .catch((err: unknown) => {
        setError(String(err));
      })
      .finally(() => {
        loadMoreInFlight.current = false;
      });
  }, []);

  return { items, setItems, loading, error, hasMore, loadMore, replaceEpoch };
}
