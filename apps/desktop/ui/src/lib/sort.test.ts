import { describe, expect, it } from 'vitest';
import {
  DEFAULT_SORT,
  SEARCH_SORT,
  directionLabels,
  effectiveSort,
  readSort,
  readSortByView,
  sortForView,
  sortInScope,
  sortKeys,
  sortWrite,
  withViewSort,
  wireSort,
  writeSort,
  type Sort,
  type SortKey,
  type SortScope,
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

/* Choosing an order writes one of two settings, and reading it back consults
   one of two rules. The pair going out of step is silent — the list stays as it
   was and the choice is simply lost — and both bugs of that shape found in
   review were here, so the round trip is asserted rather than assumed. */
describe('choosing an order, in either scope', () => {
  const chosen: Sort = { key: 'sender', ascending: true };
  type Saved = { listSort: string; listSortByView: string };
  const fresh: Saved = { listSort: writeSort(DEFAULT_SORT), listSortByView: '{}' };

  /** The settings after `chosen` is picked while looking at `view`. */
  function picked(scope: SortScope, was: Saved, view: string): Saved {
    const [setting, value] = sortWrite(scope, readSortByView(was.listSortByView), view, chosen);
    return { ...was, [setting]: value };
  }

  /** What the list is ordered by afterwards, read the way the window reads it. */
  function shown(scope: SortScope, saved: Saved, view: string): Sort {
    const shared = readSort(saved.listSort, DEFAULT_SORT);
    return sortInScope(scope, readSortByView(saved.listSortByView), view, shared);
  }

  it('gives back what was chosen, wherever it was stored', () => {
    for (const scope of ['mailbox', 'everywhere'] as SortScope[]) {
      expect(shown(scope, picked(scope, fresh, 'sent'), 'sent'), scope).toEqual(chosen);
    }
  });

  it('leaves the other mailboxes alone when each keeps its own', () => {
    const after = picked('mailbox', fresh, 'sent');
    expect(shown('mailbox', after, 'inbox')).toEqual(DEFAULT_SORT);
    // And the order everything shares was not the setting written.
    expect(after.listSort).toBe(fresh.listSort);
  });

  it('carries every list with it when they share one', () => {
    const after = picked('everywhere', fresh, 'sent');
    for (const view of ['inbox', 'sent', 'tag:Urgent']) {
      expect(shown('everywhere', after, view), view).toEqual(chosen);
    }
  });

  it('keeps the orders mailboxes were given while they are not being used', () => {
    // Off and on again gives them back: the entries stay, and `sortInScope`
    // simply stops consulting them.
    const own = picked('mailbox', fresh, 'sent');
    const shared = picked('everywhere', own, 'inbox');
    expect(shared.listSortByView).toBe(own.listSortByView);
    expect(shown('mailbox', shared, 'sent')).toEqual(chosen);
  });
});
