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
