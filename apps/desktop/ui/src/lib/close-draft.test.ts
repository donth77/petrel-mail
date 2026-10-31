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
    expect(failed).toMatchObject({ ok: false, error: 'Error: disk full' });
    const none = await settleDraft(typed, slotFor(blank), async () => null, push);
    expect(none.ok).toBe(false);
    // Nothing was written, so there is nothing to push.
    expect(push).not.toHaveBeenCalled();
  });

  it('hands over what the save threw', async () => {
    // A close the save stopped on purpose is told apart from one that failed:
    // the first has nothing to add to what the person was already told.
    const thrown = new Error('files left out');
    const push = vi.fn(async () => {});
    const result = await settleDraft(
      typed,
      slotFor(blank),
      async () => {
        throw thrown;
      },
      push,
    );
    expect(result.ok).toBe(false);
    expect(result.ok === false && result.cause).toBe(thrown);
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

  it('does not wait for the push when asked not to, but still starts it', async () => {
    // A mailbox switch: the row is safe once saved, and an APPEND over a
    // slow link must not hold the click. The push here never resolves.
    const push = vi.fn(() => new Promise<void>(() => {}));
    const result = await settleDraft(typed, slotFor(blank), async () => 5, push, {
      awaitPush: false,
    });
    expect(result).toEqual({ ok: true, id: 5 });
    expect(push).toHaveBeenCalledWith(5);
  });

  it('treats a failed push as done: the row is written and the next sync pushes it', async () => {
    const result = await settleDraft(typed, slotFor(blank), async () => 3, async () => {
      throw new Error('offline');
    });
    expect(result).toEqual({ ok: true, id: 3 });
  });
});
