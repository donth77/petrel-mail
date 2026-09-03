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
 *
 * A comma inside double quotes is part of a name, not a separator: pasting
 * `"Wu, Dana" <dana@example.com>` used to make two chips, one of them an
 * address of `"Wu`, and the send was refused for it.
 */
export function splitRecipients(field: string): string[] {
  const out: string[] = [];
  let current = '';
  let quoted = false;
  for (const ch of field) {
    if (ch === '"') {
      quoted = !quoted;
    } else if (!quoted && (ch === ',' || ch === ';')) {
      out.push(current);
      current = '';
      continue;
    }
    current += ch;
  }
  out.push(current);
  return out.map((a) => a.trim()).filter(Boolean);
}

/** The address inside a recipient entry: `Name <addr>` gives `addr`, and a
 *  bare address gives itself. What the envelope wants from the entry. */
export function addressOf(entry: string): string {
  const m = /<([^<>]*)>\s*$/.exec(entry);
  return (m ? m[1] : entry).trim();
}

/** Whether this looks like somewhere mail could go.
 *
 * Deliberately loose. This decides whether a chip is drawn as *suspect*, not
 * whether the message may be sent — an over-strict rule here would mark a
 * perfectly good address as wrong, and being told you are wrong when you are
 * not is worse than not being told. Real validation is the server's job at
 * send time, where a wrong answer is visible and recoverable.
 *
 * So: something, an @, something with a dot in it, and no spaces — judged on
 * the address inside the angle brackets when a name is written in front of
 * it, because `Dana Wu <dana@example.com>` is the form every other client
 * pastes and it is not suspect. That catches the mistakes people actually
 * make — a missing @, a trailing comma turned into an empty domain, a name
 * pasted instead of an address — and accepts everything strange but legal.
 */
export function looksLikeAddress(entry: string): boolean {
  const addr = addressOf(entry);
  const at = addr.indexOf('@');
  if (at <= 0 || at !== addr.lastIndexOf('@')) return false;
  const domain = addr.slice(at + 1);
  if (!domain.includes('.') || domain.startsWith('.') || domain.endsWith('.')) return false;
  return !/\s/.test(addr);
}
