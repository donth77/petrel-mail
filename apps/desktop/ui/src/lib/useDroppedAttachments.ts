import { useRef, type Dispatch, type SetStateAction } from 'react';
import type { Draft } from '../components/Compose';
import {
  copiedDraft,
  planDrop,
  stageDropped,
  withAttached,
  type Attached,
  type Copying,
} from './attachments';

/** One dropped file's copy: whether its chip was taken off on purpose, and
 *  whether the person has been told the file is not coming. */
type Job = { removed: boolean; told: boolean; promise: Promise<Attached> };

/** The message a save is to write, and the chips it was handed whose files
 *  it does not carry. A chip taken off on purpose is not one of them. */
export type Finished = { draft: Draft; leftOut: Copying[] };

/** Chip ids only have to differ within one window, so a count does. A random
 *  id needs a secure context: in a webview that did not count as one it
 *  would throw inside the drop, after the drop had already been taken. */
let lastChip = 0;

/**
 * Shows a dropped file's chip before the bytes have been copied, and finishes
 * the copy before a save or a send.
 *
 * The chip used to wait on `arrayBuffer` and the write to disk. Both are the
 * whole file. The name and the size are known on the drop, so the chip is
 * drawn then and the copy follows.
 */
export function useDroppedAttachments(opts: {
  getDraft: () => Draft | null;
  setDraft: Dispatch<SetStateAction<Draft | null>>;
  stage: (name: string, bytes: Uint8Array) => Promise<Attached>;
  onRejected: (names: string[]) => void;
  onFailed: (error: unknown) => void;
}) {
  // Found by the chip itself, not by its id. Every message that shows the
  // chip holds this object: the one on screen, each one a save was handed,
  // and one a failed send puts back. So a copy can be found for as long as
  // any of them can still be saved, and is let go with the last of them.
  // Kept by id and forgotten by the first save to finish, it was gone for
  // the next one: Send queued behind an autosave went without the file.
  const jobs = useRef(new WeakMap<Copying, Job>());
  const optsRef = useRef(opts);
  optsRef.current = opts;

  /**
   * A chip whose copy is over, on the message on screen: off the copying row,
   * with its file when there is one. Only while that message still shows the
   * chip, so a chip taken off stays off, an attachment taken off meanwhile is
   * not put back, and a message that never had the chip is never written into.
   */
  const land = (chip: Copying, ready: Attached | null) => {
    const shows = (d: Draft | null) => d?.copying?.some((c) => c.id === chip.id) ?? false;
    const now = optsRef.current.getDraft();
    const refused =
      ready && now && shows(now) ? withAttached(now.attachments ?? [], [ready]).rejected : [];
    optsRef.current.setDraft((cur) => {
      if (!cur || !shows(cur)) return cur;
      const copying = (cur.copying ?? []).filter((c) => c.id !== chip.id);
      if (!ready) return { ...cur, copying };
      return { ...cur, copying, attachments: withAttached(cur.attachments ?? [], [ready]).kept };
    });
    if (refused.length > 0) {
      const job = jobs.current.get(chip);
      if (job) job.told = true;
      optsRef.current.onRejected(refused);
    }
  };

  const drop = (files: readonly File[]) => {
    const current = optsRef.current.getDraft();
    if (!current) return;
    const reserved: Attached[] = [
      ...(current.attachments ?? []),
      ...(current.copying ?? []).map((c) => ({ path: c.id, name: c.name, size: c.size })),
    ];
    const plan = planDrop(
      files.map((file) => ({ name: file.name, size: file.size })),
      reserved,
    );
    if (plan.rejected.length > 0) optsRef.current.onRejected(plan.rejected);
    const added: Copying[] = [];
    for (const index of plan.take) {
      const file = files[index];
      if (!file) continue;
      lastChip += 1;
      const chip: Copying = { id: `drop-${lastChip}`, name: file.name, size: file.size };
      const base = current.attachments ?? [];
      const promise = stageDropped([file], base, optsRef.current.stage).then((result) => {
        if (result.rejected.includes(file.name) || result.kept.length === base.length) {
          optsRef.current.onRejected([file.name]);
          throw new Error('rejected');
        }
        const staged = result.kept[result.kept.length - 1];
        if (!staged) throw new Error('not staged');
        return staged;
      });
      const job: Job = { removed: false, told: false, promise };
      jobs.current.set(chip, job);
      void promise.then(
        (ready) => {
          if (!job.removed) land(chip, ready);
        },
        (error: unknown) => {
          if (job.removed) return;
          // A file refused once its bytes were counted was named as it was
          // refused; any other failure is said just below.
          job.told = true;
          land(chip, null);
          if (error instanceof Error && error.message === 'rejected') return;
          // Said even when the composer has gone. The chip was on screen, and
          // a send that was waiting on this copy has gone without the file.
          optsRef.current.onFailed(error);
        },
      );
      added.push(chip);
    }
    if (added.length === 0) return;
    optsRef.current.setDraft((cur) =>
      cur ? { ...cur, copying: [...(cur.copying ?? []), ...added] } : cur,
    );
  };

  const remove = (id: string) => {
    const chip = optsRef.current.getDraft()?.copying?.find((c) => c.id === id);
    const job = chip ? jobs.current.get(chip) : undefined;
    if (job) job.removed = true;
    optsRef.current.setDraft((cur) =>
      cur ? { ...cur, copying: (cur.copying ?? []).filter((c) => c.id !== id) } : cur,
    );
  };

  /**
   * The message with every chip it was showing either copied or left out,
   * and the chips left out other than on purpose: a copy that failed, or a
   * file that does not fit beside what the message carries. Send and the
   * like stop on those (`FilesLeftOut`) rather than take the message away
   * without a file the person saw on it.
   *
   * The person is told why once, however many saves were waiting: by the
   * copy when it fails, by `land` when the file lands on screen and does not
   * fit, and here when it landed with no message on screen to say so on.
   */
  const finish = async (draft: Draft): Promise<Finished> => {
    const chips = draft.copying ?? [];
    const over = new Map<string, Attached | null>();
    const ready = await copiedDraft(draft, async (id) => {
      const chip = chips.find((c) => c.id === id);
      const job = chip ? jobs.current.get(chip) : undefined;
      let file: Attached | null = null;
      if (job && !job.removed) {
        try {
          const copied = await job.promise;
          file = job.removed ? null : copied;
        } catch {
          file = null;
        }
      }
      over.set(id, file);
      return file;
    });
    // A message a failed send put back still shows chips whose copies ended
    // while it was away; the copy had no message on screen to land on then.
    for (const chip of chips) land(chip, over.get(chip.id) ?? null);
    // A copied file can still be refused for size: something attached while
    // it copied was judged without it.
    const carried = new Set((ready.attachments ?? []).map((a) => a.path));
    const leftOut = chips.filter((chip) => {
      const file = over.get(chip.id);
      return !(file && carried.has(file.path)) && !jobs.current.get(chip)?.removed;
    });
    const untold: Copying[] = [];
    for (const chip of leftOut) {
      const job = jobs.current.get(chip);
      if (!job || job.told) continue;
      job.told = true;
      untold.push(chip);
    }
    if (untold.length > 0) optsRef.current.onRejected(untold.map((c) => c.name));
    return { draft: ready, leftOut };
  };

  return { drop, remove, finish };
}
