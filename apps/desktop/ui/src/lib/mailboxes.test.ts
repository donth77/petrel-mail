import { describe, expect, it } from 'vitest';
import {
  ESSENTIAL,
  arrangementFor,
  MAILBOX_KEYS,
  countFor,
  countModes,
  defaultCount,
  parseArrangement,
  serialiseArrangement,
  shipped,
  visibleMailboxes,
} from './mailboxes';

describe('what a mailbox counts by default', () => {
  /* The same one rule the engine states: a list you built by hand counts
     everything on it, a place mail lands by itself counts what you have not
     read, and nothing waits in Sent. */
  it('counts everything on a list you made yourself', () => {
    for (const key of ['starred', 'snoozed', 'drafts', 'outbox']) {
      expect(defaultCount(key), key).toBe('total');
    }
  });

  it('counts the unread where mail lands by itself', () => {
    for (const key of ['inbox', 'archive', 'spam', 'trash', 'folders']) {
      expect(defaultCount(key), key).toBe('unread');
    }
  });

  it('counts nothing in Sent', () => {
    expect(defaultCount('sent')).toBe('off');
  });
});

describe('reading a stored arrangement', () => {
  it('ships in order, with nothing hidden', () => {
    expect(shipped().order).toEqual([...MAILBOX_KEYS]);
    expect(shipped().hidden).toEqual([]);
  });

  it('falls back rather than throwing on nonsense', () => {
    // A bad string in the settings table must not be why the sidebar stops
    // drawing. Every one of these is something a corrupt or older value
    // could actually be.
    for (const bad of ['', '   ', 'not json', 'null', '[]', '{"order":"inbox"}', '{"hidden":7}']) {
      expect(parseArrangement(bad).order, bad).toEqual([...MAILBOX_KEYS]);
    }
  });

  it('keeps the order somebody chose', () => {
    const a = parseArrangement('{"order":["trash","inbox"]}');
    expect(a.order.slice(0, 2)).toEqual(['trash', 'inbox']);
  });

  it('appends a mailbox the stored order never mentioned, rather than raising it', () => {
    // The rule folders already follow: arranged rows first, then the rest.
    const a = parseArrangement('{"order":["trash","inbox"]}');
    expect(a.order).toHaveLength(MAILBOX_KEYS.length);
    expect(a.order.indexOf('spam')).toBeGreaterThan(1);
  });

  it('drops a key it does not recognise', () => {
    const a = parseArrangement('{"order":["inbox","gopher"],"hidden":["gopher"]}');
    expect(a.order).not.toContain('gopher');
    expect(a.hidden).not.toContain('gopher');
  });

  it('refuses to hide the inbox, however it was asked', () => {
    const a = parseArrangement('{"hidden":["inbox","spam"]}');
    expect(a.hidden).toEqual(['spam']);
    expect(visibleMailboxes(a)).toContain(ESSENTIAL);
  });

  it('ignores a count mode that is not one', () => {
    const a = parseArrangement('{"counts":{"inbox":"sideways","spam":"off"}}');
    expect(a.counts.inbox).toBeUndefined();
    expect(a.counts.spam).toBe('off');
  });
});

describe('writing one back', () => {
  it('stores only what differs, so improved defaults still reach people', () => {
    const a = shipped();
    a.counts = { starred: 'total', spam: 'off' }; // the first is already the default
    const stored = JSON.parse(serialiseArrangement(a));
    expect(stored.counts).toEqual({ spam: 'off' });
  });

  it('round-trips an arrangement somebody made', () => {
    const a = shipped();
    a.order = ['trash', ...MAILBOX_KEYS.filter((k) => k !== 'trash')];
    a.hidden = ['snoozed'];
    a.counts = { inbox: 'total' };
    const back = parseArrangement(serialiseArrangement(a));
    expect(back.order).toEqual(a.order);
    expect(back.hidden).toEqual(['snoozed']);
    expect(countFor(back, 'inbox')).toBe('total');
  });
});

describe('what the rail and the counts query are handed', () => {
  it('leaves out what is hidden, and never the inbox', () => {
    const a = shipped();
    a.hidden = ['spam', 'snoozed'];
    const shown = visibleMailboxes(a);
    expect(shown).not.toContain('spam');
    expect(shown).toContain('inbox');
    expect(shown).toHaveLength(MAILBOX_KEYS.length - 2);
  });

  it('still asks for a hidden mailbox’s number', () => {
    // Or unhiding one shows an empty row until the next recount.
    const a = shipped();
    a.hidden = ['spam'];
    expect(countModes(a).spam).toBe('unread');
  });

  it('answers for folders too, which have no row of their own', () => {
    expect(countModes(shipped()).folders).toBe('unread');
  });
});

describe('the setting this replaces', () => {
  it('keeps counts off for somebody who had turned them off', () => {
    const a = arrangementFor('', 'off');
    expect(countFor(a, 'inbox')).toBe('off');
    expect(countFor(a, 'folders')).toBe('off');
  });

  it('keeps totals for somebody who had asked for everything', () => {
    expect(countFor(arrangementFor('', 'total'), 'inbox')).toBe('total');
  });

  it('uses the per-mailbox rule for everyone else', () => {
    const a = arrangementFor('', 'unread');
    expect(countFor(a, 'inbox')).toBe('unread');
    expect(countFor(a, 'starred')).toBe('total');
    expect(countFor(a, 'sent')).toBe('off');
  });

  it('is ignored once the sidebar has been arranged', () => {
    // The old switch must not keep overriding a choice made after it.
    const a = arrangementFor('{"counts":{"inbox":"total"}}', 'off');
    expect(countFor(a, 'inbox')).toBe('total');
    expect(countFor(a, 'spam')).toBe('unread');
  });
});
