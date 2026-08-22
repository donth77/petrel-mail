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
