import { describe, expect, it } from 'vitest';
import { chips, hasToken, scopedQuery, toggleToken, tokensOf } from './search-chips';

describe('tokensOf', () => {
  it('keeps a quoted value whole', () => {
    expect(tokensOf('from:"Dana Wu" annex')).toEqual(['from:Dana Wu', 'annex']);
  });
});

describe('toggleToken', () => {
  it('adds a token to what is already typed', () => {
    expect(toggleToken('annex', 'has:attachment')).toBe('annex has:attachment');
  });

  it('takes it away again', () => {
    expect(toggleToken('annex has:attachment', 'has:attachment')).toBe('annex');
  });

  it('adds to an empty field', () => {
    expect(toggleToken('', 'is:unread')).toBe('is:unread');
  });

  /* Two senders is a query that matches nothing, and nobody means it. */
  it('replaces a different value for the same operator', () => {
    expect(toggleToken('from:sam annex', 'from:dana')).toBe('annex from:dana');
  });

  it('leaves the words alone either way', () => {
    const on = toggleToken('quarterly report', 'is:starred');
    expect(on).toContain('quarterly');
    expect(on).toContain('report');
    expect(toggleToken(on, 'is:starred')).toBe('quarterly report');
  });

  /* `is:` conditions are separate booleans in the engine, so they narrow
     together — unlike from:/in:/after:, where a second token overwrites the
     first. Treating them all as one-value operators made a chip in the
     Snoozed view throw the view away. */
  it('keeps the states that narrow together', () => {
    expect(toggleToken('is:snoozed', 'is:unread')).toBe('is:snoozed is:unread');
    expect(toggleToken('is:starred', 'is:unread')).toBe('is:starred is:unread');
    expect(toggleToken('is:unread', 'has:attachment')).toBe('is:unread has:attachment');
  });

  it('never drops the scope the search started from', () => {
    expect(toggleToken('in:Receipts', 'is:unread')).toBe('in:Receipts is:unread');
    expect(toggleToken('in:Receipts', 'has:attachment')).toBe('in:Receipts has:attachment');
    expect(toggleToken('is:snoozed', 'has:attachment')).toBe('is:snoozed has:attachment');
  });

  it('still replaces the operators the engine reads as one value', () => {
    expect(toggleToken('in:inbox', 'in:sent')).toBe('in:sent');
    expect(toggleToken('after:2025 annex', 'after:2026')).toBe('annex after:2026');
  });
});

describe('hasToken', () => {
  it('lights a chip from the field, not from a state of its own', () => {
    expect(hasToken('annex has:attachment', 'has:attachment')).toBe(true);
    expect(hasToken('annex', 'has:attachment')).toBe(false);
  });

  /* Typing the operator by hand must light the chip too — otherwise the two
     halves of the same control disagree about what is being searched. */
  it('recognises an operator that was typed rather than clicked', () => {
    expect(hasToken('is:unread', 'is:unread')).toBe(true);
    expect(hasToken('IS:UNREAD', 'is:unread')).toBe(true);
  });

  it('does not mistake a longer token for this one', () => {
    expect(hasToken('has:attachments', 'has:attachment')).toBe(false);
  });
});

describe('chips', () => {
  it('offers the sender only when there is one', () => {
    expect(chips(null, 2026, 'inbox').some((c) => c.id === 'from')).toBe(false);
    expect(chips('Sam Ortiz', 2026, 'inbox').find((c) => c.id === 'from')?.token)
      .toBe('from:"Sam Ortiz"');
  });

  it('quotes a name with a space and leaves a single word bare', () => {
    expect(chips('sam', 2026, 'inbox').find((c) => c.id === 'from')?.token).toBe('from:sam');
  });
});

describe('the scope chip', () => {
  const scope = (view: string) => chips(null, 2026, view).find((c) => c.id === 'scope');

  it('names the mailbox you are actually in', () => {
    expect(scope('inbox')).toEqual({ id: 'scope', label: 'In Inbox', token: 'in:inbox' });
    expect(scope('archive')).toEqual({ id: 'scope', label: 'In Archive', token: 'in:archive' });
    expect(scope('sent')?.token).toBe('in:sent');
  });

  it('offers the way into spam and trash, which search otherwise leaves out', () => {
    expect(scope('spam')?.token).toBe('in:spam');
    expect(scope('trash')?.token).toBe('in:trash');
  });

  it('speaks is: for the state views and stays silent only where it must', () => {
    expect(scope('starred')?.token).toBe('is:starred');
    expect(scope('snoozed')?.token).toBe('is:snoozed');
    expect(scope('outbox')).toBeUndefined();
    expect(scope('tag:Urgent')).toBeUndefined();
  });

  it('does not double the starred chip when the scope already is it', () => {
    const ids = chips(null, 2026, 'starred').map((c) => c.id);
    expect(ids.filter((i) => i === 'starred' || i === 'scope')).toEqual(['scope']);
  });

  it('comes first: the pre-applied context leads the row', () => {
    const ids = chips('Sam', 2026, 'sent').map((c) => c.id);
    expect(ids[0]).toBe('scope');
    expect(ids).toContain('from');
  });

  it('names a user folder by its leaf, quoted when it has spaces', () => {
    expect(chips(null, 2026, 'folder:7', 'Receipts')[0].token).toBe('in:Receipts');
    expect(chips(null, 2026, 'folder:9', 'Client contact')[0].token).toBe('in:"Client contact"');
  });
});

/* What is on gathers at the left. A row that reads lit, unlit, lit, unlit
   makes the reader scan for the answer to "what am I filtering by"; grouped,
   the answer is the first run of chips. */
describe('the order applied chips take', () => {
  it('puts what is on before what is off', () => {
    const ids = chips(null, 2026, 'inbox', null, 'in:inbox is:starred').map((c) => c.id);
    expect(ids.slice(0, 2)).toEqual(['scope', 'starred']);
  });

  it('lifts a chip out of the middle rather than leaving a gap', () => {
    // `unread` sits third in the built row; applied, it comes second.
    const ids = chips(null, 2026, 'inbox', null, 'in:inbox is:unread').map((c) => c.id);
    expect(ids).toEqual(['scope', 'unread', 'attachment', 'starred', 'year']);
  });

  it('keeps the built order inside each group', () => {
    const ids = chips('Sam', 2026, 'sent', null, 'has:attachment after:2026').map((c) => c.id);
    // Applied, in the order they were built; then the rest, likewise.
    expect(ids).toEqual(['attachment', 'year', 'scope', 'from', 'unread', 'starred']);
  });

  it('leads with an applied chip even when the scope was deleted', () => {
    // Deleting the scope token is how a search goes global; the row must not
    // keep a dark chip at the front while a lit one sits behind it.
    const ids = chips(null, 2026, 'inbox', null, 'is:unread').map((c) => c.id);
    expect(ids[0]).toBe('unread');
  });

  it('changes nothing while the row is empty', () => {
    const ids = chips('Sam', 2026, 'sent', null, '').map((c) => c.id);
    expect(ids).toEqual(['scope', 'from', 'attachment', 'unread', 'starred', 'year']);
  });
});

describe('scopedQuery', () => {
  it('scopes a beginning search to wherever you stand', () => {
    expect(scopedQuery('a', '', 'in:spam')).toBe('in:spam a');
    expect(scopedQuery('a', '', 'in:inbox')).toBe('in:inbox a');
    expect(scopedQuery('a', '', 'in:"Client contact"')).toBe('in:"Client contact" a');
  });

  it('never re-applies mid-edit, so deleting the token widens', () => {
    expect(scopedQuery('in:inbox a', 'in:inbox ', 'in:inbox')).toBe('in:inbox a');
    expect(scopedQuery('a', 'in:inbox a', 'in:inbox')).toBe('a');
    expect(scopedQuery('a', '', null)).toBe('a');
  });

  it('only writes the token as the search begins', () => {
    // Otherwise deleting it would fight the person deleting it.
    expect(scopedQuery('in:spam refun', 'in:spam refund', 'spam')).toBe('in:spam refun');
    expect(scopedQuery('refund', 'refun', 'spam')).toBe('refund');
  });

  it('does nothing when the field is being cleared', () => {
    expect(scopedQuery('', 'refund', 'spam')).toBe('');
    expect(scopedQuery('   ', '', 'spam')).toBe('   ');
  });
});

/* A filter in the query must always have a pill. Without one there is no way
   to see what is narrowing the list, and no way to take it off. */
describe('a filter that is applied always has its chip', () => {
  it('keeps the From chip when the search changed what is open', () => {
    // The From chip was built from the open conversation, so running the
    // search — which empties the selection — took the pill away while
    // `from:Slack` went on filtering.
    const ids = chips(null, 2026, 'inbox', null, 'in:inbox from:Slack is:unread').map((c) => c.id);
    expect(ids).toContain('from');
  });

  it('names that chip after the query, not after whatever is open now', () => {
    const from = chips('Someone Else', 2026, 'inbox', null, 'from:Slack').find(
      (c) => c.id === 'from',
    );
    expect(from?.token).toBe('from:Slack');
    expect(from?.label).toBe('From Slack');
  });

  it('lights a chip whose value is quoted', () => {
    expect(hasToken('in:"Client contact" is:unread', 'in:"Client contact"')).toBe(true);
    expect(hasToken('from:"Dana Wu"', 'from:"Dana Wu"')).toBe(true);
  });

  it('does not lose the quotes when another chip is toggled', () => {
    // Splitting the query and joining it back turned `in:"Client contact"`
    // into two words, and the search then meant something else entirely.
    expect(toggleToken('in:"Client contact"', 'is:unread')).toBe(
      'in:"Client contact" is:unread',
    );
  });

  it('can take a quoted chip off again', () => {
    expect(toggleToken('in:"Client contact" is:unread', 'in:"Client contact"')).toBe('is:unread');
  });

  it('names the scope chip after the mailbox being searched, not the one on screen', () => {
    // Standing in the Inbox with `in:Receipts` typed, the row used to offer
    // "In Inbox" unlit while `in:Receipts` narrowed the list with no pill.
    const scope = chips(null, 2026, 'inbox', null, 'in:Receipts is:unread').find(
      (c) => c.id === 'scope',
    );
    expect(scope?.token).toBe('in:Receipts');
    expect(scope?.label).toBe('In Receipts');
  });

  it('still offers the open mailbox when nothing scopes the query', () => {
    const scope = chips(null, 2026, 'inbox', null, 'is:unread').find((c) => c.id === 'scope');
    expect(scope?.token).toBe('in:inbox');
    expect(scope?.label).toBe('In Inbox');
  });

  it('keeps the friendly name when the query scopes where you already are', () => {
    const scope = chips(null, 2026, 'inbox', null, 'in:inbox').find((c) => c.id === 'scope');
    expect(scope?.label).toBe('In Inbox');
  });
});
