import type { Draft } from '../components/Compose';
import { unsaved, type ComposerSlot } from './draft-autosave';

/** Whether the composer may go. A failed save says why, and hands over what
 *  was thrown (`cause`) for a caller that tells one failure from another. */
export type Settled =
  | { ok: true; id: number | null }
  | { ok: false; error: string; cause?: unknown };

/**
 * Puts a composer's message away before the composer goes.
 *
 * Whatever is unsaved is written first; the row is then pushed so its server
 * copy does not wait out the push debounce. The answer says whether the
 * composer may now go: a message that could not be written must stay on
 * screen, because closing over a failed save is the one way a composer loses
 * what somebody typed. A message that never held anything has no row and
 * nothing to push, and may go at once.
 *
 * Shared by the popped-out window's close, by the account switch in the
 * main window, which has to file the draft under the account it was written
 * in before the store's notion of "active" moves, and by leaving Drafts for
 * another mailbox, which used to leave the editor floating over the inbox.
 */
export async function settleDraft(
  d: Draft,
  slot: ComposerSlot,
  save: (d: Draft) => Promise<number | null>,
  push: (id: number) => Promise<void>,
  opts: { awaitPush?: boolean } = {},
): Promise<Settled> {
  let id = slot.id;
  if (unsaved(d, slot)) {
    try {
      id = await save(d);
    } catch (e) {
      return { ok: false, error: String(e), cause: e };
    }
    if (id == null) return { ok: false, error: 'no draft row' };
  }
  // Best effort: the row is written either way, and the next sync pass
  // pushes whatever this one could not. A mailbox switch does not wait for
  // the server: the words are safe once the row is written, and an APPEND
  // over a slow link must not hold the click on Inbox.
  if (id != null) {
    const pushed = push(id).catch(() => {});
    if (opts.awaitPush !== false) await pushed;
  }
  return { ok: true, id };
}
