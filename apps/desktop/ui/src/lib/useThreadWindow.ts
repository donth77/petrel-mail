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

/** What a load was asked for, so its answer can be checked against what the
 *  window wants by the time it lands. The generation counts every replaced
 *  window — account, query, view or sort — so a page for a list since left
 *  is never folded into the one showing now. The view and sort are checked
 *  as well as the generation because the merge and the page are not started
 *  by the replace effect, and only the generation ties them to it. */
export type Asked = { gen: number; view: string; sort: Sort };

export function stillWanted(asked: Asked, now: Asked): boolean {
  return asked.gen === now.gen && asked.view === now.view && asked.sort === now.sort;
}

/** Where a background load puts its answer. The rows it reads and writes
 *  are the window's; a failure is reported and changes nothing else. */
export type WindowSink = {
  items: () => Thread[];
  setItems: (next: Thread[] | ((prev: Thread[]) => Thread[])) => void;
  setHasMore: (more: boolean) => void;
  bumpReplace: () => void;
  /** A background load that could not be made. The rows on screen are still
   *  the rows; the notice says the newest may be missing. */
  failed: (error: string) => void;
};

/** Folds a fresh first page into the loaded window.
 *
 *  Its failure keeps the rows. A refresh that fails used to put the whole
 *  list behind an error notice — forty conversations gone because one poll
 *  could not get a page — when everything on screen was still true. */
export async function refreshHead(
  fetchers: ThreadFetchers,
  view: string,
  sort: Sort,
  wanted: () => boolean,
  sink: WindowSink,
): Promise<void> {
  try {
    const rows = await fetchers.threads(...firstPageCall(view, sort));
    if (!wanted()) return;
    const wasEmpty = sink.items().length === 0;
    sink.setItems((cur) =>
      mergeHead(cur, rows, { byDate: sort.key === 'date', ascending: sort.ascending }),
    );
    if (wasEmpty) {
      sink.setHasMore(rows.length === LIST_PAGE);
      sink.bumpReplace();
    }
  } catch (err: unknown) {
    if (wanted()) sink.failed(String(err));
  }
}

/** Appends the page after `last`. Same rule as the head merge: a page for a
 *  window since left is dropped, and a failure keeps what is loaded. */
export async function pageMore(
  fetchers: ThreadFetchers,
  view: string,
  sort: Sort,
  last: Thread,
  wanted: () => boolean,
  sink: WindowSink,
): Promise<void> {
  try {
    const rows = await fetchers.threads(...loadMoreCall(view, sort, last));
    if (!wanted()) return;
    const { items: next, reachedEnd } = appendPage(sink.items(), rows);
    sink.setItems(next);
    if (reachedEnd) sink.setHasMore(false);
  } catch (err: unknown) {
    if (wanted()) sink.failed(String(err));
  }
}

export function useThreadWindow(args: {
  query: string;
  view: string;
  sort: Sort;
  accountEpoch: number;
  /** Live message count. Increases mean new mail — merge into the head, never replace the loaded window. */
  messageCount: number | undefined;
  fetchers: ThreadFetchers;
  /** A background page or refresh that failed. The rows stay; this is where
   *  the failure is said. */
  onRefreshFailed?: (error: string) => void;
}): {
  items: Thread[];
  setItems: React.Dispatch<React.SetStateAction<Thread[]>>;
  loading: boolean;
  /** Why the window could not be loaded at all. Only a replace load sets
   *  it — a window that never arrived — and the next replace clears it. */
  error: string | null;
  hasMore: boolean;
  loadMore: () => void;
  /** Bumps when the loaded window is replaced (view, query, sort, account)
   *  or when the first mail lands in an empty list. Paging and new mail at
   *  the head do not bump it — the highlight must not jump just because
   *  the array is new. */
  replaceEpoch: number;
} {
  const { query, view, sort, accountEpoch, messageCount, fetchers, onRefreshFailed } = args;

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

  const failedRef = useRef(onRefreshFailed);
  failedRef.current = onRefreshFailed;

  const loadMoreInFlight = useRef(false);
  const messageCountRef = useRef(messageCount);
  const [replaceEpoch, setReplaceEpoch] = useState(0);
  // Counts replaced windows. Every load remembers the generation it was
  // started under and is dropped if the window has been replaced since.
  const gen = useRef(0);

  const asked = useCallback(
    (): Asked => ({ gen: gen.current, view: viewRef.current, sort: sortRef.current }),
    [],
  );

  const sink = useRef<WindowSink>({
    items: () => itemsRef.current,
    setItems: (next) => setItems(next),
    setHasMore: (more) => setHasMore(more),
    bumpReplace: () => setReplaceEpoch((n) => n + 1),
    failed: (e) => failedRef.current?.(e),
  });

  // Replace the window when the mailbox, query, sort, or account changes.
  useEffect(() => {
    let live = true;
    gen.current += 1;
    const myGen = gen.current;
    setLoading(true);
    // A failure belongs to the window that failed. Left standing, it hid
    // the next window too, even when that one loaded.
    setError(null);

    const debounceMs = query.trim() ? 100 : 0;
    const handle = window.setTimeout(() => {
      runReplaceLoad(fetchersRef.current, query, view, sort)
        .then(({ items: rows, hasMore: more }) => {
          if (!live || gen.current !== myGen) return;
          setItems(rows);
          setHasMore(more);
          setReplaceEpoch((n) => n + 1);
          setLoading(false);
        })
        .catch((err: unknown) => {
          if (!live || gen.current !== myGen) return;
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
    // The answer belongs to the window asked for. A page for the inbox that
    // arrived after a click on Sent used to be merged into Sent, and a short
    // one replaced it outright; one for the last account did the same.
    const was = asked();
    void refreshHead(
      fetchersRef.current,
      was.view,
      was.sort,
      () => live && stillWanted(was, asked()),
      sink.current,
    );

    return () => {
      live = false;
    };
  }, [messageCount, query, asked]);

  const loadMore = useCallback(() => {
    if (queryRef.current.trim() || !hasMoreRef.current || loadMoreInFlight.current) return;

    const current = itemsRef.current;
    const last = current[current.length - 1];
    if (!last) return;

    loadMoreInFlight.current = true;
    const was = asked();
    void pageMore(
      fetchersRef.current,
      was.view,
      was.sort,
      last,
      () => stillWanted(was, asked()),
      sink.current,
    ).finally(() => {
      loadMoreInFlight.current = false;
    });
  }, [asked]);

  return { items, setItems, loading, error, hasMore, loadMore, replaceEpoch };
}
