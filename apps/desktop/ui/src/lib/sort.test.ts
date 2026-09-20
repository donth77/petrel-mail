import { describe, expect, it } from 'vitest';
import {
  DEFAULT_SORT,
  SEARCH_SORT,
  directionLabels,
  effectiveSort,
  readSort,
  readSortByView,
  sortForView,
  sortKeys,
  withViewSort,
  wireSort,
  writeSort,
  type Sort,
  type SortKey,
} from './sort';

describe('what a list can be ordered by', () => {
  it('offers relevance only to a search', () => {
    expect(sortKeys(true)).toContain('relevance');
    expect(sortKeys(false)).not.toContain('relevance');
  });

  it('offers the same three keys either way', () => {
    for (const key of ['date', 'sender', 'subject'] as const) {
      expect(sortKeys(false), key).toContain(key);
      expect(sortKeys(true), key).toContain(key);
    }
  });
});

describe('naming the two directions', () => {
  /* "Ascending" is a word about numbers. A list of names sorted ascending is
     a list sorted A to Z, and saying so is the difference between a control
     you read and one you experiment with. */
  it('talks about time for dates and about letters for the rest', () => {
    expect(directionLabels('date')).toEqual({
      ascending: 'sort-oldest',
      descending: 'sort-newest',
    });
    expect(directionLabels('sender')).toEqual({ ascending: 'sort-a-z', descending: 'sort-z-a' });
    expect(directionLabels('subject')).toEqual({ ascending: 'sort-a-z', descending: 'sort-z-a' });
  });
});

describe('relevance when the search ends', () => {
  it('falls back rather than leaving a mailbox claiming an order it cannot have', () => {
    const relevance: Sort = { key: 'relevance', ascending: false };
    expect(effectiveSort(relevance, false)).toEqual(DEFAULT_SORT);
    expect(effectiveSort(relevance, true)).toEqual(relevance);
  });

  it('leaves a real key alone either way', () => {
    const bySender: Sort = { key: 'sender', ascending: true };
    expect(effectiveSort(bySender, false)).toEqual(bySender);
    expect(effectiveSort(bySender, true)).toEqual(bySender);
  });
});

describe('what the engine is told', () => {
  it('sends no key for relevance, because it is the absence of one', () => {
    expect(wireSort({ key: 'relevance', ascending: false }).key).toBeUndefined();
  });

  it('sends the key and the direction for everything else', () => {
    expect(wireSort({ key: 'subject', ascending: true })).toEqual({
      key: 'subject',
      ascending: true,
    });
  });
});

describe('a sort, remembered', () => {
  it('reads back as itself', () => {
    for (const key of ['relevance', 'date', 'sender', 'subject'] as SortKey[]) {
      for (const ascending of [true, false]) {
        const sort = { key, ascending };
        expect(readSort(writeSort(sort), DEFAULT_SORT)).toEqual(sort);
      }
    }
  });

  it('falls back on anything it does not know', () => {
    // The fallback is a key `readSort` cannot produce from these, so a stub
    // that ignored its input could not pass this.
    const fallback = { key: 'sender', ascending: true } as const;
    for (const said of ['', 'nonsense', 'colour:ascending', ':', 'relevance ascending']) {
      expect(readSort(said, fallback)).toEqual(fallback);
    }
    // A key it knows, with a direction it does not, is that key descending.
    expect(readSort('date', fallback)).toEqual({ key: 'date', ascending: false });
    expect(readSort('date:sideways', fallback)).toEqual({ key: 'date', ascending: false });
    expect(readSort('subject:ascending', fallback)).toEqual({ key: 'subject', ascending: true });
  });
});

describe('a mailbox remembering its own order', () => {
  const byView = { inbox: 'sender:ascending', 'tag:Urgent': 'subject:descending' };

  it('gives a view its own order, and the rest the one the window keeps', () => {
    expect(sortForView(byView, 'inbox', DEFAULT_SORT)).toEqual({ key: 'sender', ascending: true });
    expect(sortForView(byView, 'tag:Urgent', DEFAULT_SORT)).toEqual({
      key: 'subject',
      ascending: false,
    });
    expect(sortForView(byView, 'sent', DEFAULT_SORT)).toEqual(DEFAULT_SORT);
    expect(sortForView({}, 'inbox', SEARCH_SORT)).toEqual(SEARCH_SORT);
  });

  it('changes one view and leaves the others alone', () => {
    const after = readSortByView(withViewSort(byView, 'sent', { key: 'date', ascending: true }));
    expect(after.sent).toBe('date:ascending');
    expect(after.inbox).toBe('sender:ascending');
  });

  it('survives a setting that says something else entirely', () => {
    for (const said of ['', 'null', '[]', 'not json', '{"inbox":5}', '{"inbox":{"key":"date"}}']) {
      expect(sortForView(readSortByView(said), 'inbox', DEFAULT_SORT)).toEqual(DEFAULT_SORT);
    }
  });
});
