import { describe, expect, it } from 'vitest';
import {
  MAX_OPEN_BODIES,
  THREAD_PAGE_MAX,
  bodiesToMount,
  clampThreadLimit,
  keepExistingPane,
  nextExpanded,
  olderCards,
  previewCard,
} from './reader-window';
import type { Thread } from './api';

describe('keepExistingPane', () => {
  it('holds the cards only for the thread already on screen', () => {
    expect(keepExistingPane({ loadedThreadId: 5, requestedThreadId: 5 })).toBe(true);
    expect(keepExistingPane({ loadedThreadId: 5, requestedThreadId: 6 })).toBe(false);
    expect(keepExistingPane({ loadedThreadId: null, requestedThreadId: 5 })).toBe(false);
  });
});

describe('clampThreadLimit', () => {
  it('clamps below one and above the IPC cap', () => {
    expect(clampThreadLimit(0)).toBe(1);
    expect(clampThreadLimit(-5)).toBe(1);
    expect(clampThreadLimit(THREAD_PAGE_MAX + 50)).toBe(THREAD_PAGE_MAX);
  });
});

describe('nextExpanded', () => {
  it('never exceeds the iframe cap', () => {
    const out = nextExpanded({ prev: new Set([1, 2, 3]), add: 4, newestId: 10 });
    expect(out.size).toBeLessThanOrEqual(MAX_OPEN_BODIES);
  });

  it('keeps the newest and the row just opened', () => {
    const out = nextExpanded({ prev: new Set([1, 2, 3]), add: 4, newestId: 3 });
    expect(out.has(4)).toBe(true);
    expect(out.has(3)).toBe(true);
  });

  it('stays capped after many walks', () => {
    let expanded = new Set<number>([10]);
    for (let i = 1; i <= 20; i += 1) {
      expanded = nextExpanded({ prev: expanded, add: i, newestId: 10 });
      expect(expanded.size).toBeLessThanOrEqual(MAX_OPEN_BODIES);
    }
  });
});

describe('bodiesToMount', () => {
  it('mounts at most three bodies', () => {
    const expanded = new Set([1, 2, 3, 4]);
    expect(bodiesToMount(expanded, 4).size).toBeLessThanOrEqual(MAX_OPEN_BODIES);
  });
});

describe('previewCard', () => {
  const thread: Thread = {
    thread_id: 10,
    id: 42,
    from_display: 'Sam',
    from_addr: 'sam@example.com',
    subject: 'Hello',
    snippet: 'Hi',
    date_ms: 1,
    message_count: 3,
    participants: 'Sam',
    unread: true,
    starred: false,
    has_attachments: false,
    tags: [],
    attachment_name: null,
    match_snippet: null,
  };

  it('names the listing newest so a body can mount before the index', () => {
    expect(previewCard(thread)).toEqual({
      id: 42,
      from_display: 'Sam',
      from_addr: 'sam@example.com',
      snippet: 'Hi',
      date_ms: 1,
      unread: true,
    });
  });

  it('is enough for the newest body slot', () => {
    const seed = previewCard(thread);
    expect(bodiesToMount(new Set([seed.id]), seed.id).has(seed.id)).toBe(true);
  });
});

describe('olderCards', () => {
  const card = (id: number) => ({
    id,
    from_display: '',
    from_addr: '',
    snippet: '',
    date_ms: id,
    unread: false,
  });

  it('is empty when the conversation is one message', () => {
    expect(olderCards({ index: [card(42)], newestId: 42 })).toEqual([]);
  });

  it('keeps older rows in index order and drops the newest', () => {
    expect(olderCards({ index: [card(1), card(2), card(42)], newestId: 42 }).map((c) => c.id)).toEqual(
      [1, 2],
    );
  });

  it('drops the newest wherever it sits', () => {
    expect(olderCards({ index: [card(1), card(42), card(3)], newestId: 42 }).map((c) => c.id)).toEqual(
      [1, 3],
    );
  });

  it('is empty when the index has not arrived', () => {
    expect(olderCards({ index: [], newestId: 42 })).toEqual([]);
  });
});
