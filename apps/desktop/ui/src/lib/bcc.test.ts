import { describe, expect, it } from 'vitest';
import type { DraftRecord, ThreadMessage } from './api';
import { draftHasContent, draftSignature } from './draft-autosave';
import { draftFromRecord } from './draft-record';
import { firstUnsendable, hasRecipient } from './recipients';
import { replyTargets } from './reply';

/* Blind copies, in the composer's own bookkeeping. The wire and the store
   are proved in Rust; these are the decisions the window makes before
   anything reaches either. */

describe('who a message can be sent to', () => {
  it('counts blind copies: a message to a list of parents has only those', () => {
    expect(hasRecipient({ to: '', cc: '', bcc: 'priya@example.net' })).toBe(true);
  });

  it('counts Cc too, as every other client does', () => {
    expect(hasRecipient({ to: '', cc: 'alex@example.com' })).toBe(true);
  });

  it('is nobody when every field is blank or only separators', () => {
    expect(hasRecipient({ to: '', cc: '' })).toBe(false);
    expect(hasRecipient({ to: ' , ', cc: ';', bcc: '  ' })).toBe(false);
  });
});

describe('a half-typed blind copy', () => {
  it('stops the send, as a half-typed To does', () => {
    expect(firstUnsendable({ to: 'dana@example.com', cc: '', bcc: 'Priya Nair' })).toBe(
      'Priya Nair',
    );
    expect(firstUnsendable({ to: 'dana@example.com', cc: '', bcc: 'priya@example.net' })).toBe(
      null,
    );
  });
});

describe('saving blind copies', () => {
  const blank = { to: '', cc: '', subject: '', body: '', html: '' };

  it('counts a Bcc line as something worth keeping', () => {
    expect(draftHasContent({ ...blank, bcc: 'priya@example.net' })).toBe(true);
  });

  it('saves again when only the Bcc changed', () => {
    expect(draftSignature({ ...blank, bcc: 'priya@example.net' })).not.toBe(
      draftSignature({ ...blank, bcc: 'board@example.org' }),
    );
  });

  it('reads a message from before Bcc the same as one with an empty Bcc', () => {
    expect(draftSignature(blank)).toBe(draftSignature({ ...blank, bcc: '' }));
  });
});

describe('a draft read back from the store', () => {
  const record = (envelope: DraftRecord['envelope']): DraftRecord => ({
    id: 7,
    to: 'dana@example.com',
    cc: '',
    subject: 'Numbers',
    body: '',
    html: '',
    envelope,
    files: [],
  });

  it('brings its blind copies', () => {
    const d = draftFromRecord(
      record({
        in_reply_to: null,
        references: [],
        attachments: [],
        bcc: 'priya@example.net, board@example.org',
      }),
    );
    expect(d.bcc).toBe('priya@example.net, board@example.org');
  });

  it('has none when it was saved before there was a Bcc', () => {
    const d = draftFromRecord(record({ in_reply_to: null, references: [], attachments: [] }));
    expect(d.bcc).toBe('');
  });
});

describe('replying to a message you blind-copied people on', () => {
  const mine = {
    id: 1,
    from_display: 'You',
    from_addr: 'you@example.com',
    subject: 'Numbers',
    snippet: '',
    date_ms: 0,
    unread: false,
    to: ['Dana Wu'],
    cc: ['alex@example.com'],
    bcc: ['Priya Nair'],
    recipients: ['Dana Wu', 'alex@example.com'],
    recipient_addrs: ['dana@example.com', 'alex@example.com'],
    to_addrs: ['dana@example.com'],
    cc_addrs: ['alex@example.com'],
    attachments: [],
  } as unknown as ThreadMessage;

  it('writes to the people written to openly and never to a blind copy', () => {
    for (const all of [false, true]) {
      const { to, cc } = replyTargets(mine, 'you@example.com', all);
      expect([...to, ...cc].join(' ')).not.toMatch(/priya/i);
      if (all) {
        expect(to).toEqual(['dana@example.com']);
        expect(cc).toEqual(['alex@example.com']);
      } else {
        expect(to).toEqual(['dana@example.com', 'alex@example.com']);
        expect(cc).toEqual([]);
      }
    }
  });
});
