import { describe, expect, it, vi } from 'vitest';
import { LIST_PAGE } from './list-page';
import { DEFAULT_SORT } from './sort';
import type { Thread } from './api';
import {
  appendPage,
  firstPageCall,
  loadMoreCall,
  mergeHead,
  replaceLoadHasMore,
  runReplaceLoad,
  type ThreadFetchers,
} from './useThreadWindow';

let nextId = 1;

function thread(over: Partial<Thread> & Pick<Thread, 'thread_id'>): Thread {
  const id = over.id ?? nextId++;
  return {
    id,
    thread_id: over.thread_id,
    from_display: over.from_display ?? 'Sender',
    from_addr: over.from_addr ?? 'sender@example.com',
    subject: over.subject ?? 'Subject',
    snippet: over.snippet ?? 'Snippet',
    date_ms: over.date_ms ?? 1_000_000 - over.thread_id,
    message_count: over.message_count ?? 1,
    participants: over.participants ?? '',
    unread: over.unread ?? false,
    starred: over.starred ?? false,
    has_attachments: over.has_attachments ?? false,
    tags: over.tags ?? [],
    attachment_name: over.attachment_name ?? null,
    match_snippet: over.match_snippet ?? null,
  };
}

function threadsFetcher(pages: Record<string, Thread[][]>): ThreadFetchers['threads'] {
  return (
    view,
    offset,
    limit,
    sort,
    ascending,
    beforeDateMs,
    beforeThreadId,
  ) => {
    void offset;
    void sort;
    void ascending;
    const key =
      beforeDateMs === undefined && beforeThreadId === undefined
        ? `${view}:head`
        : `${view}:${beforeDateMs}:${beforeThreadId}`;
    const stack = pages[key] ?? [];
    const page = stack.shift() ?? [];
    void limit;
    return Promise.resolve(page);
  };
}

describe('firstPageCall', () => {
  it('asks for offset zero, LIST_PAGE rows, and no cursor', () => {
    expect(firstPageCall('inbox', DEFAULT_SORT)).toEqual([
      'inbox',
      0,
      LIST_PAGE,
      'date',
      false,
    ]);
  });
});

describe('loadMoreCall', () => {
  it('passes the last row as the keyset cursor', () => {
    const last = thread({ thread_id: 9, date_ms: 42 });
    expect(loadMoreCall('inbox', DEFAULT_SORT, last)).toEqual([
      'inbox',
      0,
      LIST_PAGE,
      'date',
      false,
      42,
      9,
    ]);
  });
});

describe('appendPage', () => {
  it('appends without duplicate thread_ids', () => {
    const prev = [thread({ thread_id: 1 }), thread({ thread_id: 2 })];
    const incoming = [
      thread({ thread_id: 2, subject: 'dup' }),
      thread({ thread_id: 3 }),
    ];
    const { items, reachedEnd } = appendPage(prev, incoming);
    expect(items.map((t) => t.thread_id)).toEqual([1, 2, 3]);
    expect(items[1].subject).toBe(prev[1].subject);
    expect(reachedEnd).toBe(true);
  });

  it('reports the end when the page is short', () => {
    const prev = [thread({ thread_id: 1 })];
    const incoming = Array.from({ length: LIST_PAGE - 1 }, (_, i) =>
      thread({ thread_id: i + 10 }),
    );
    expect(appendPage(prev, incoming).reachedEnd).toBe(true);
  });

  it('keeps hasMore when a full page arrives', () => {
    const prev = [thread({ thread_id: 1 })];
    const incoming = Array.from({ length: LIST_PAGE }, (_, i) =>
      thread({ thread_id: i + 10 }),
    );
    expect(appendPage(prev, incoming).reachedEnd).toBe(false);
  });
});

describe('mergeHead', () => {
  const page = (ids: number[], first = 10_000) =>
    ids.map((tid, i) => thread({ thread_id: tid, date_ms: first - i }));
  const byDate = { byDate: true, ascending: false };

  it('leads with the fresh page and keeps the tail it does not cover', () => {
    const tail = thread({ thread_id: 300, subject: 'old tail', date_ms: 1 });
    const prev = [thread({ thread_id: 2, subject: 'stale', unread: true, date_ms: 9_999 }), tail];
    const incoming = page([1, 2, ...Array.from({ length: LIST_PAGE - 2 }, (_, i) => i + 3)]);
    const merged = mergeHead(prev, incoming, byDate);
    expect(merged.length).toBe(LIST_PAGE + 1);
    expect(merged[0].thread_id).toBe(1);
    expect(merged[1].subject).toBe('Subject');
    expect(merged[1].unread).toBe(false);
    expect(merged[merged.length - 1]).toBe(tail);
  });

  it('moves a conversation with a new reply to where the fresh page puts it', () => {
    const prev = page([5, 6, 7]);
    const incoming = page([7, 5, 6, ...Array.from({ length: LIST_PAGE - 3 }, (_, i) => i + 8)]);
    const merged = mergeHead(prev, incoming, byDate);
    expect(merged.slice(0, 3).map((t) => t.thread_id)).toEqual([7, 5, 6]);
    expect(merged.filter((t) => t.thread_id === 7).length).toBe(1);
  });

  it('drops a tail row the fresh page should have covered, by date', () => {
    const incoming = page(Array.from({ length: LIST_PAGE }, (_, i) => i + 1));
    const edge = incoming[incoming.length - 1].date_ms;
    const gone = thread({ thread_id: 900, date_ms: edge + 5 });
    const kept = thread({ thread_id: 901, date_ms: edge - 5 });
    const merged = mergeHead([gone, kept], incoming, byDate);
    expect(merged.some((t) => t.thread_id === 900)).toBe(false);
    expect(merged[merged.length - 1]).toBe(kept);
    // Not by date: there is no range to judge by, so both stay.
    const bySender = mergeHead([gone, kept], incoming, { byDate: false, ascending: false });
    expect(bySender.length).toBe(LIST_PAGE + 2);
  });

  it('treats a short page as the whole view', () => {
    const prev = page([1, 2, 3]);
    const incoming = page([1, 3]);
    expect(mergeHead(prev, incoming, byDate).map((t) => t.thread_id)).toEqual([1, 3]);
  });
});

describe('runReplaceLoad', () => {
  it('loads the first listing page for an empty query', async () => {
    const rows = Array.from({ length: LIST_PAGE }, (_, i) => thread({ thread_id: i + 1 }));
    const threads = vi.fn(async () => rows);
    const search = vi.fn(async () => []);
    const fetchers: ThreadFetchers = { threads, search };

    const result = await runReplaceLoad(fetchers, '', 'inbox', DEFAULT_SORT);

    expect(threads).toHaveBeenCalledWith(...firstPageCall('inbox', DEFAULT_SORT));
    expect(search).not.toHaveBeenCalled();
    expect(result.items).toEqual(rows);
    expect(result.hasMore).toBe(true);
  });

  it('searches instead of listing when there is a query', async () => {
    const rows = [thread({ thread_id: 1 })];
    const threads = vi.fn(async () => rows);
    const search = vi.fn(async () => rows);
    const fetchers: ThreadFetchers = { threads, search };

    const result = await runReplaceLoad(fetchers, 'invoice', 'inbox', DEFAULT_SORT);

    expect(search).toHaveBeenCalledWith('invoice', 'date', false);
    expect(threads).not.toHaveBeenCalled();
    expect(result.hasMore).toBe(false);
  });

  it('replaces the list on a view change rather than merging', async () => {
    const inbox = [thread({ thread_id: 1, subject: 'inbox' })];
    const archive = [thread({ thread_id: 9, subject: 'archive' })];
    const fetchers: ThreadFetchers = {
      threads: threadsFetcher({
        'inbox:head': [inbox],
        'archive:head': [archive],
      }),
      search: async () => [],
    };

    const first = await runReplaceLoad(fetchers, '', 'inbox', DEFAULT_SORT);
    const second = await runReplaceLoad(fetchers, '', 'archive', DEFAULT_SORT);

    expect(first.items[0].subject).toBe('inbox');
    expect(second.items[0].subject).toBe('archive');
    expect(second.items.some((t) => t.thread_id === 1)).toBe(false);
  });

  it('clears hasMore when the first page is short', async () => {
    const fetchers: ThreadFetchers = {
      threads: async () => [thread({ thread_id: 1 })],
      search: async () => [],
    };
    const result = await runReplaceLoad(fetchers, '', 'inbox', DEFAULT_SORT);
    expect(replaceLoadHasMore('', result.items.length)).toBe(false);
    expect(result.hasMore).toBe(false);
  });
});

describe('loadMore integration', () => {
  it('requests the cursor from the last row and appends new pages', async () => {
    const page1 = Array.from({ length: LIST_PAGE }, (_, i) =>
      thread({ thread_id: i + 1, date_ms: 10_000 - i }),
    );
    const page2 = [thread({ thread_id: LIST_PAGE + 1, date_ms: 0 })];
    const threads = threadsFetcher({
      'inbox:head': [page1],
      [`inbox:${page1[page1.length - 1].date_ms}:${page1[page1.length - 1].thread_id}`]: [page2],
    });
    const fetchers: ThreadFetchers = { threads, search: async () => [] };

    const first = await runReplaceLoad(fetchers, '', 'inbox', DEFAULT_SORT);
    const last = first.items[first.items.length - 1];
    const more = await fetchers.threads(...loadMoreCall('inbox', DEFAULT_SORT, last));
    const { items, reachedEnd } = appendPage(first.items, more);

    expect(loadMoreCall('inbox', DEFAULT_SORT, last)).toEqual([
      'inbox',
      0,
      LIST_PAGE,
      'date',
      false,
      last.date_ms,
      last.thread_id,
    ]);
    expect(items.length).toBe(LIST_PAGE + 1);
    expect(reachedEnd).toBe(true);
  });
});
