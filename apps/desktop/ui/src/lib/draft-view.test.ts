import { describe, expect, it } from 'vitest';
import { opensComposer } from './draft-view';

describe('opensComposer', () => {
  it('is true only for the drafts mailbox', () => {
    expect(opensComposer('drafts')).toBe(true);
    expect(opensComposer('inbox')).toBe(false);
    expect(opensComposer('sent')).toBe(false);
    expect(opensComposer('outbox')).toBe(false);
    expect(opensComposer('folder:3')).toBe(false);
  });
});
