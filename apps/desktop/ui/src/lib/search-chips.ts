/**
 * The filter chips above the search field.
 *
 * They write into the query, they do not replace it. Clicking "Has attachment"
 * types `has:attachment` where you can see it, edit it, or delete it — someone
 * who never learns the grammar gets buttons, someone who does gets the same
 * thing faster, and neither is fighting a filter held somewhere they cannot
 * reach. A chip is lit because the token is in the field, not because a
 * checkbox is ticked in a state of its own.
 */

/** Whether the query already carries this exact operator. */
export function hasToken(query: string, token: string): boolean {
  return tokensOf(query).some((t) => t.toLowerCase() === token.toLowerCase());
}

/** Splits a query the way the engine does, keeping `from:"Dana Wu"` whole. */
export function tokensOf(query: string): string[] {
  const out: string[] = [];
  let current = '';
  let quoted = false;
  for (const ch of query) {
    if (ch === '"') quoted = !quoted;
    else if (/\s/.test(ch) && !quoted) {
      if (current) out.push(current);
      current = '';
    } else current += ch;
  }
  if (current) out.push(current);
  return out;
}

/**
 * Adds the token, or takes it away if it is already there.
 *
 * A chip for `from:` replaces any existing `from:` rather than adding a second
 * one — two senders is a query that matches nothing, and nobody means it.
 */
export function toggleToken(query: string, token: string): string {
  const key = token.includes(':') ? `${token.split(':')[0].toLowerCase()}:` : null;
  const tokens = tokensOf(query);

  if (hasToken(query, token)) {
    return tokens.filter((t) => t.toLowerCase() !== token.toLowerCase()).join(' ');
  }

  // A different value for the same operator: replace rather than accumulate.
  const kept = key
    ? tokens.filter((t) => !t.toLowerCase().startsWith(key))
    : tokens;
  return [...kept, token].join(' ').trim();
}

/** The chips on offer, in the order the mockup shows them. */
export type Chip = { id: string; label: string; token: string };

export function chips(sender: string | null, year: number): Chip[] {
  const list: Chip[] = [];
  // The sender of whatever is open, because "more from this person" is the
  // search people actually run — and it is tedious to type.
  if (sender) {
    const value = /\s/.test(sender) ? `"${sender}"` : sender;
    list.push({ id: 'from', label: `From ${sender}`, token: `from:${value}` });
  }
  list.push(
    { id: 'attachment', label: 'Has attachment', token: 'has:attachment' },
    { id: 'unread', label: 'Unread', token: 'is:unread' },
    { id: 'starred', label: 'Starred', token: 'is:starred' },
    { id: 'year', label: 'This year', token: `after:${year}` },
    { id: 'inbox', label: 'In Inbox', token: 'in:inbox' },
  );
  return list;
}
