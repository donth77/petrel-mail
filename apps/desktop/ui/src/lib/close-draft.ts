import type { Draft } from '../components/Compose';
import { unsaved, type ComposerSlot } from './draft-autosave';

export type Settled = { ok: true; id: number | null } | { ok: false; error: string };

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
 * Shared by the popped-out window's close, and by the account switch in the
 * main window, which has to file the draft under the account it was written
 * in before the store's notion of "active" moves.
 */
export async function settleDraft(
  d: Draft,
  slot: ComposerSlot,
  save: (d: Draft) => Promise<number | null>,
  push: (id: number) => Promise<void>,
): Promise<Settled> {
  let id = slot.id;
  if (unsaved(d, slot)) {
    try {
      id = await save(d);
    } catch (e) {
      return { ok: false, error: String(e) };
    }
    if (id == null) return { ok: false, error: 'no draft row' };
  }
  // Best effort: the row is written either way, and the next sync pass
  // pushes whatever this one could not.
  if (id != null) await push(id).catch(() => {});
  return { ok: true, id };
}
