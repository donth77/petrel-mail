import { describe, expect, it } from 'vitest';
import type { DraftRecord } from './api';
import { fits } from './attachments';
import { draftFromRecord } from './draft-record';

const record = (over: Partial<DraftRecord> & Pick<DraftRecord, 'id'>): DraftRecord => ({
  to: '',
  cc: '',
  subject: '',
  body: '',
  html: '',
  envelope: { in_reply_to: null, references: [], attachments: [] },
  files: [],
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

describe('a reopened draft’s files', () => {
  const MB = 1024 * 1024;
  const report = '/data/staged/1-0/report.pdf';
  const reopened = (size: number) =>
    draftFromRecord(
      record({
        id: 4,
        envelope: { in_reply_to: null, references: [], attachments: [report] },
        files: [{ path: report, name: 'report.pdf', size }],
      }),
    );

  it('come back with their size, not 0 B', () => {
    expect(reopened(15 * MB).attachments).toEqual([
      { path: report, name: 'report.pdf', size: 15 * MB },
    ]);
  });

  it('count against the attachment limit', () => {
    // Encoded, 15 MB is over 20 MB: another 15 MB does not fit beside it.
    expect(fits(reopened(15 * MB).attachments ?? [], 15 * MB)).toBe(false);
    expect(fits(reopened(1 * MB).attachments ?? [], 15 * MB)).toBe(true);
  });

  it('keep a file the backend could not size, named from its path', () => {
    const d = draftFromRecord(
      record({
        id: 5,
        envelope: { in_reply_to: null, references: [], attachments: [report] },
      }),
    );
    expect(d.attachments).toEqual([{ path: report, name: 'report.pdf', size: 0 }]);
  });
});
