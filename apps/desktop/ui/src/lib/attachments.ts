/** What a file costs on the wire once base64-encoded.
 *
 *  Three bytes become four, wrapped every 76 characters — about 37% larger
 *  than the file on disk. Checking a limit against the size in Finder lets
 *  someone attach something apparently under it and watch the send fail, which
 *  is the worst moment to learn the number was wrong.
 *
 *  Mirrors encoded_size in petrel-providers; the two are asserted equal by the
 *  Rust tests and this comment. */
export function encodedSize(rawBytes: number): number {
  const base64 = Math.ceil(rawBytes / 3) * 4;
  return base64 + Math.ceil(base64 / 76) * 2;
}

/** Gmail's ceiling, and the lowest of the common providers.
 *
 *  A constant rather than the server's advertised SIZE, which SMTP does offer
 *  in its EHLO reply — reading it means an SMTP round trip before the composer
 *  opens, so this stays a documented default until that exists. Erring low is
 *  the safe direction: refusing something that would have squeezed through is
 *  recoverable, and a send that fails after the fact is not. */
export const ATTACHMENT_LIMIT = 25 * 1024 * 1024;

export type Attached = { path: string; name: string; size: number };

/** A chip shown the moment a file is dropped, before its bytes are copied.
 *
 *  The copy is what takes the time. The name and the size are known without
 *  it, and the chip is the thing someone is waiting to see. */
export type Copying = { id: string; name: string; size: number };

/** Whether one more file fits, counting what is already attached. */
export function fits(existing: Attached[], addition: number): boolean {
  const used = existing.reduce((n, a) => n + encodedSize(a.size), 0);
  return used + encodedSize(addition) <= ATTACHMENT_LIMIT;
}

/**
 * Runs the file picker and works out what may be kept.
 *
 * Shared by the docked composer and the popped-out one so a pop-out is not
 * quietly a lesser composer — and so the size rule has one definition rather
 * than two that can disagree about what fits.
 *
 * Returns null when the picker was cancelled, which is an answer rather than
 * a failure and must not be reported as one.
 *
 * The picker is the shell's, not the webview's. A path only ever reaches a
 * command from a panel the shell itself opened, so nothing rendered from a
 * message can name a file on disk.
 */
export async function pickAttachments(
  existing: readonly Attached[],
  info: (paths: string[]) => Promise<Attached[]>,
  pick: () => Promise<string[]>,
): Promise<{ kept: Attached[]; rejected: string[] } | null> {
  const picked = await pick();
  if (picked.length === 0) return null;

  return withAttached(existing, await info(picked));
}

/**
 * Files added to what a message already carries: each once, and only while
 * they fit. The new ones come after the old, so `kept.slice(existing.length)`
 * is what this added.
 *
 * Also how a composer folds in what a picker or a drop produced once the
 * await is over. Written back as `{ ...draft, attachments }` from a copy
 * taken before it, that put back whatever had changed meanwhile — typing, a
 * subject, the row id a save had just learned — and with the composer closed
 * by then it replaced the message with a bare list, and the window crashed.
 * Applied to the message as it is now, against what it carries now, none of
 * that can happen.
 */
export function withAttached(
  existing: readonly Attached[],
  added: readonly Attached[],
): { kept: Attached[]; rejected: string[] } {
  const kept = [...existing];
  const rejected: string[] = [];
  for (const f of added) {
    if (kept.some((a) => a.path === f.path)) continue;
    if (!fits(kept, f.size)) {
      rejected.push(f.name);
      continue;
    }
    kept.push(f);
  }
  return { kept, rejected };
}

/**
 * Which dropped files can be shown at once.
 *
 * Decided from the name and the size alone. Reading the bytes is what makes
 * a drop feel late, and a file that cannot fit is refused here so it is never
 * read at all. Sizes already taken, including chips still copying, count.
 */
export function planDrop(
  files: readonly { name: string; size: number }[],
  existing: readonly Attached[],
): { take: number[]; rejected: string[] } {
  const kept = [...existing];
  const take: number[] = [];
  const rejected: string[] = [];
  files.forEach((file, index) => {
    if (!fits(kept, file.size)) {
      rejected.push(file.name);
      return;
    }
    // Reserve this file's size so the next one in the same drop is judged
    // against it, not against an empty message.
    kept.push({ path: `drop:${index}`, name: file.name, size: file.size });
    take.push(index);
  });
  return { take, rejected };
}

/**
 * The draft as it can be saved: chips that were waiting on a copy are either
 * real files now, or gone because the copy was cancelled or failed.
 *
 * `take` returns null for a chip that must not be kept. Fields other than
 * the attachment lists are left as they were.
 */
export async function copiedDraft<T extends { attachments?: Attached[]; copying?: Copying[] }>(
  draft: T,
  take: (id: string) => Promise<Attached | null>,
): Promise<T> {
  const pending = draft.copying ?? [];
  if (pending.length === 0) return draft;
  const landed: Attached[] = [];
  for (const item of pending) {
    const ready = await take(item.id);
    if (ready) landed.push(ready);
  }
  return {
    ...draft,
    copying: [],
    attachments: withAttached(draft.attachments ?? [], landed).kept,
  };
}

/**
 * Takes files dragged in from the desktop.
 *
 * Shares the size rule and the de-duplication with `pickAttachments` for the
 * same reason that function is shared at all: a file that is too large is too
 * large however it arrived, and two definitions of "fits" would eventually
 * disagree.
 *
 * The size is checked before the file is written down, not after. A 400MB video
 * dropped by accident should be refused, not copied into the application's
 * own storage first and refused second.
 */
export async function stageDropped(
  files: readonly File[],
  existing: readonly Attached[],
  stage: (name: string, bytes: Uint8Array) => Promise<Attached>,
): Promise<{ kept: Attached[]; rejected: string[] }> {
  const kept = [...existing];
  const rejected: string[] = [];
  for (const file of files) {
    if (!fits(kept, file.size)) {
      rejected.push(file.name);
      continue;
    }
    const bytes = new Uint8Array(await file.arrayBuffer());
    // The size on the drop can be a guess. The bytes are the size that counts,
    // and they are not written down when they do not fit.
    if (!fits(kept, bytes.byteLength)) {
      rejected.push(file.name);
      continue;
    }
    kept.push(await stage(file.name, bytes));
  }
  return { kept, rejected };
}
