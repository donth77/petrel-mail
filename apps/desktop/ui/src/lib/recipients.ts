/** Recipient strings, split and judged.
 *
 * The composer keeps recipients as one comma-separated string because that is
 * what drafts, replies and the send path all speak. These are the two questions
 * the chip field asks of it.
 */

/** What ends one recipient and starts the next, outside quotes and `<…>`. */
const SEPARATORS = new Set([',', ';', '\n', '\r']);

/** Splits a recipient field, forgiving the separators people actually type.
 *
 * Deliberately tolerant: someone pasting from a calendar invite or a signature
 * gets semicolons, and someone typing gets commas and stray spaces. Rejecting
 * their paste teaches them to distrust the field rather than teaching them the
 * separator.
 *
 * A line break separates too. A column of addresses copied from a spreadsheet
 * or another client arrives one per line, and Apple Mail, Gmail and
 * Thunderbird all make a recipient of each line. Here the three became one
 * red chip, and Send refused it as a single address.
 *
 * A comma inside double quotes is part of a name: pasting
 * `"Wu, Dana" <dana@example.com>` used to make two chips, one of them an
 * address of `"Wu`, and the send was refused for it. Nothing inside `<…>`
 * splits either, since that is the address itself. A quote or a `<` protects
 * only when it closes, and a `<` closes at the next `>`, before any other
 * `<`: one typed and never closed swallowed every recipient after it into a
 * single red chip. The send path splits by the same rule (`split_recipients`
 * in send.rs), so the chips on screen are the recipients on the envelope.
 */
export function splitRecipients(field: string): string[] {
  const chars = [...field];
  // Where a quote or a `<` opened that never closed: read as a plain
  // character from then on. Each pass finds at most one more.
  const plain = new Set<number>();
  for (;;) {
    const pass = splitPass(chars, plain);
    if (typeof pass === 'number') {
      plain.add(pass);
      continue;
    }
    return pass.map((a) => a.trim()).filter(Boolean);
  }
}

/** One reading of the field: its entries, or where the first quote or `<`
 *  that never closes was opened. */
function splitPass(chars: string[], plain: Set<number>): string[] | number {
  const out: string[] = [];
  let current = '';
  let quoteAt = -1;
  let bracketAt = -1;
  for (let i = 0; i < chars.length; i += 1) {
    const ch = chars[i];
    if (plain.has(i)) {
      current += ch;
      continue;
    }
    if (ch === '"') {
      quoteAt = quoteAt < 0 ? i : -1;
    } else if (quoteAt < 0 && ch === '<') {
      // An address holds no `<`, so the one before never closed.
      if (bracketAt >= 0) return bracketAt;
      bracketAt = i;
    } else if (quoteAt < 0 && ch === '>') {
      bracketAt = -1;
    } else if (quoteAt < 0 && bracketAt < 0 && SEPARATORS.has(ch)) {
      out.push(current);
      current = '';
      continue;
    }
    current += ch;
  }
  if (quoteAt >= 0) return quoteAt;
  if (bracketAt >= 0) return bracketAt;
  out.push(current);
  return out;
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

/** Whether the sender would put this entry on the envelope at all.
 *
 * The rule the wire applies, no looser and no stricter: an address with an
 * @ and nothing in it that would make it two addresses or two lines. This is
 * the one that decides whether a message may go — `looksLikeAddress` above
 * is only advice. An entry that fails here would be left off the envelope
 * silently, and the message would reach everyone else with nobody told.
 */
export function sendable(entry: string): boolean {
  const addr = addressOf(entry);
  if (addr.length === 0 || !addr.includes('@')) return false;
  for (const ch of addr) {
    const code = ch.charCodeAt(0);
    // A control character would start a second line on the wire; the rest
    // would make this two addresses.
    if (code < 0x20 || code === 0x7f) return false;
    if (/[\s<>,;]/.test(ch)) return false;
  }
  return true;
}

/** The first To, Cc or Bcc entry the wire would drop, or null when every
 *  entry would go. What a send checks before it does anything else. */
export function firstUnsendable(d: { to: string; cc: string; bcc?: string }): string | null {
  for (const entry of [
    ...splitRecipients(d.to),
    ...splitRecipients(d.cc),
    ...splitRecipients(d.bcc ?? ''),
  ]) {
    if (!sendable(entry)) return entry;
  }
  return null;
}

/** Whether a message names anyone at all to send to: To, Cc or Bcc.
 *
 *  Any of the three will do, as in every other client: a note to a list of
 *  parents goes in Bcc and nowhere else. The send path agrees, counting the
 *  same three on the envelope. */
export function hasRecipient(d: { to: string; cc: string; bcc?: string }): boolean {
  return [d.to, d.cc, d.bcc ?? ''].some((field) => splitRecipients(field).length > 0);
}

/** Which recipient field a chip is dragged between. */
export type RecipientField = 'to' | 'cc' | 'bcc';

/** One confirmed chip moving from one field to another.
 *
 * `addr` is the chip string itself, not a normalised address. The field is a
 * list of those strings, and two writings of one person are two chips. */
export type RecipientMove = {
  from: RecipientField;
  to: RecipientField;
  index: number;
  addr: string;
};

/** Moves one confirmed chip from one field to another.
 *
 * Returns null when there is nothing to do: the same field (chips are not
 * reordered, and the strings are not rebuilt), an index that is not a chip,
 * or a chip string that is no longer the one at that index. The caller then
 * leaves the draft alone, so a rejected move keeps whatever spacing the field
 * already had — a double space and all.
 *
 * Otherwise that one index leaves the source. It is appended to the
 * destination unless that exact string is already there; the source still
 * loses it. A chip that does not look like an address moves too. It was
 * confirmed, and the dashed outline is only a doubt. The two fields that
 * changed are joined the same way a committed chip is. The field that was
 * not touched is returned as it stood. */
export function moveRecipient(
  fields: { to: string; cc: string; bcc: string },
  move: RecipientMove,
): { to: string; cc: string; bcc: string } | null {
  if (move.from === move.to) return null;
  const source = splitRecipients(fields[move.from]);
  if (move.index < 0 || move.index >= source.length) return null;
  if (source[move.index] !== move.addr) return null;
  const dest = splitRecipients(fields[move.to]);
  const kept = source.filter((_, i) => i !== move.index);
  const nextDest = dest.includes(move.addr) ? dest : [...dest, move.addr];
  return {
    ...fields,
    [move.from]: kept.join(', '),
    [move.to]: nextDest.join(', '),
  };
}