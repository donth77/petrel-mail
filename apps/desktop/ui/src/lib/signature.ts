import type { Identity } from './api';

/**
 * The body a new message or reply starts with.
 *
 * The separator is "-- " exactly — two hyphens, a space, then a newline. That
 * is what every mail client looks for to fold a signature away, and getting it
 * subtly wrong (no space, three hyphens) means the signature is shown as
 * ordinary text at the bottom of every message forever.
 */
export const SEPARATOR = '-- ';

export function startingBody(identity: Identity | null, isReply: boolean): string {
  if (!identity?.signature.trim()) return '';
  if (isReply && !identity.signature_on_reply) return '';
  // Blank lines above it so the cursor lands somewhere to write, rather than
  // immediately on top of the signature.
  return `\n\n${SEPARATOR}\n${identity.signature}`;
}

/** Escapes text for placing inside HTML. */
function escapeHtml(text: string): string {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;');
}

/**
 * The same starting body, as the HTML the editor is seeded with.
 *
 * The separator survives as literal text rather than becoming an `<hr>`: the
 * folding every client does is a string match on "-- " at the start of a line,
 * and a horizontal rule is not that string however much it looks like one.
 *
 * Two empty paragraphs above it, for the same reason the text version has two
 * newlines — the caret should land somewhere to write, not on the signature.
 */
export function startingHtml(identity: Identity | null, isReply: boolean): string {
  if (!identity?.signature.trim()) return '';
  if (isReply && !identity.signature_on_reply) return '';
  const lines = identity.signature
    .split('\n')
    .map((line) => `<p>${escapeHtml(line) || '<br>'}</p>`)
    .join('');
  return `<p></p><p></p><p>${escapeHtml(SEPARATOR)}</p>${lines}`;
}
