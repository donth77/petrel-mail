/** Recipient strings, split and judged.
 *
 * The composer keeps recipients as one comma-separated string because that is
 * what drafts, replies and the send path all speak. These are the two questions
 * the chip field asks of it.
 */

/** Splits a recipient field, forgiving the separators people actually type.
 *
 * Deliberately tolerant: someone pasting from a calendar invite or a signature
 * gets semicolons, and someone typing gets commas and stray spaces. Rejecting
 * their paste teaches them to distrust the field rather than teaching them the
 * separator.
 */
export function splitRecipients(field: string): string[] {
  return field
    .split(/[,;]/)
    .map((a) => a.trim())
    .filter(Boolean);
}

/** Whether this looks like somewhere mail could go.
 *
 * Deliberately loose. This decides whether a chip is drawn as *suspect*, not
 * whether the message may be sent — an over-strict rule here would mark a
 * perfectly good address as wrong, and being told you are wrong when you are
 * not is worse than not being told. Real validation is the server's job at
 * send time, where a wrong answer is visible and recoverable.
 *
 * So: something, an @, something with a dot in it, and no spaces. That catches
 * the mistakes people actually make — a missing @, a trailing comma turned into
 * an empty domain, a name pasted instead of an address — and accepts everything
 * strange but legal.
 */
export function looksLikeAddress(addr: string): boolean {
  const at = addr.indexOf('@');
  if (at <= 0 || at !== addr.lastIndexOf('@')) return false;
  const domain = addr.slice(at + 1);
  if (!domain.includes('.') || domain.startsWith('.') || domain.endsWith('.')) return false;
  return !/\s/.test(addr);
}
