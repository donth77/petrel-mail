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
 */
export async function pickAttachments(
  existing: readonly Attached[],
  info: (paths: string[]) => Promise<Attached[]>,
): Promise<{ kept: Attached[]; rejected: string[] } | null> {
  const { open } = await import('@tauri-apps/plugin-dialog');
  const picked = await open({ multiple: true });
  if (!picked) return null;

  const files = await info(Array.isArray(picked) ? picked : [picked]);
  const kept = [...existing];
  const rejected: string[] = [];
  for (const f of files) {
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
    kept.push(await stage(file.name, bytes));
  }
  return { kept, rejected };
}
