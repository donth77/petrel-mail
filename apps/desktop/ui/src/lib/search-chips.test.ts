import { describe, expect, it } from 'vitest';
import { chips, hasToken, toggleToken, tokensOf } from './search-chips';

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

  it('offers nothing where `in:` could not express it', () => {
    // A chip writing `in:starred` or `in:Urgent` would match nothing at all,
    // which is worse than no chip: it looks like a filter that found nothing.
    for (const view of ['starred', 'snoozed', 'outbox', 'tag:Urgent', 'settings']) {
      expect(scope(view), view).toBeUndefined();
    }
  });

  it('comes last, after the conditions', () => {
    const ids = chips(null, 2026, 'inbox').map((c) => c.id);
    expect(ids[ids.length - 1]).toBe('scope');
  });
});
