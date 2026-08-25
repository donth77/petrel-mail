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
