import { describe, expect, it, vi } from 'vitest';
import { settleDraft } from './close-draft';
import { draftSignature, slotFor } from './draft-autosave';

const blank = { to: '', cc: '', subject: '', body: '', html: '' };
const typed = { ...blank, to: 'sam@example.com', body: 'hello' };

describe('settleDraft', () => {
  it('writes what is unsaved, pushes the row, and lets the composer go', async () => {
    const save = vi.fn(async () => 7);
    const push = vi.fn(async () => {});
    const result = await settleDraft(typed, slotFor(blank), save, push);
    expect(save).toHaveBeenCalledWith(typed);
    expect(push).toHaveBeenCalledWith(7);
    expect(result).toEqual({ ok: true, id: 7 });
  });

  it('keeps the composer open when the save fails', async () => {
    const push = vi.fn(async () => {});
    const failed = await settleDraft(
      typed,
      slotFor(blank),
      async () => {
        throw new Error('disk full');
      },
      push,
    );
    expect(failed).toEqual({ ok: false, error: 'Error: disk full' });
    const none = await settleDraft(typed, slotFor(blank), async () => null, push);
    expect(none.ok).toBe(false);
    // Nothing was written, so there is nothing to push.
    expect(push).not.toHaveBeenCalled();
  });

  it('only pushes a message that is already saved', async () => {
    const save = vi.fn(async () => 9);
    const push = vi.fn(async () => {});
    const slot = { id: 12, signature: draftSignature(typed) };
    const result = await settleDraft(typed, slot, save, push);
    expect(save).not.toHaveBeenCalled();
    expect(push).toHaveBeenCalledWith(12);
    expect(result).toEqual({ ok: true, id: 12 });
  });

  it('lets an empty composer go without writing a row', async () => {
    const save = vi.fn(async () => 1);
    const push = vi.fn(async () => {});
    const result = await settleDraft(blank, slotFor(blank), save, push);
    expect(save).not.toHaveBeenCalled();
    expect(push).not.toHaveBeenCalled();
    expect(result).toEqual({ ok: true, id: null });
  });

  it('treats a failed push as done: the row is written and the next sync pushes it', async () => {
    const result = await settleDraft(typed, slotFor(blank), async () => 3, async () => {
      throw new Error('offline');
    });
    expect(result).toEqual({ ok: true, id: 3 });
  });
});
