import { describe, expect, it } from 'vitest';
import type { DraftRecord } from './api';
import { draftFromRecord } from './draft-record';

const record = (over: Partial<DraftRecord> & Pick<DraftRecord, 'id'>): DraftRecord => ({
  to: '',
  cc: '',
  subject: '',
  body: '',
  html: '',
  envelope: { in_reply_to: null, references: [], attachments: [] },
  ...over,
});

describe('draftFromRecord', () => {
  it('preserves cc', () => {
    const d = draftFromRecord(record({ id: 1, cc: 'a@example.com, b@example.com' }));
    expect(d.cc).toBe('a@example.com, b@example.com');
  });

  it('keeps empty cc empty', () => {
    const d = draftFromRecord(record({ id: 2, cc: '' }));
    expect(d.cc).toBe('');
  });

  it('preserves inReplyTo from the envelope', () => {
    const d = draftFromRecord(
      record({
        id: 3,
        envelope: { in_reply_to: '<msg-42@example.com>', references: [], attachments: [] },
      }),
    );
    expect(d.inReplyTo).toBe('<msg-42@example.com>');
  });
});
