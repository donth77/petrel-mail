import { describe, expect, it } from 'vitest';
import {
  DEFAULT_SORT,
  directionLabels,
  effectiveSort,
  sortKeys,
  wireSort,
  type Sort,
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
