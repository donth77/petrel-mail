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

/**
 * The mailboxes a scope chip can name.
 *
 * `in:` resolves against folder roles, so this is the whole of what it can
 * express. Starred is a flag and tags are a table of their own — neither is a
 * folder, so neither gets a scope chip rather than getting one that silently
 * matches nothing. Starred already has a chip of its own further up, which is
 * the same narrowing by another route.
 */
const SCOPES: Record<string, string> = {
  inbox: 'Inbox',
  archive: 'Archive',
  sent: 'Sent',
  drafts: 'Drafts',
  spam: 'Spam',
  trash: 'Trash',
};

/**
 * @param view The mailbox on screen, which the scope chip offers to narrow to.
 *   Search itself is never scoped: a search that quietly covered only the
 *   mailbox you happened to be standing in would answer "no results" for mail
 *   you do have, and nothing on screen would say why. Narrowing stays a visible
 *   token you added and can delete.
 */
export function chips(sender: string | null, year: number, view: string): Chip[] {
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
  );
  // Last, and named for where you actually are. Spam and Trash are the useful
  // case as much as Inbox: search leaves both out unless asked, and this is the
  // asking.
  const scope = SCOPES[view];
  if (scope) list.push({ id: 'scope', label: `In ${scope}`, token: `in:${view}` });
  return list;
}

/**
 * The one place a search has to be narrowed for you.
 *
 * Search leaves Spam and Trash out of every result, which is right everywhere
 * except standing inside them: there, an unscoped search returns nothing at all
 * and the mailbox is simply not searchable. Being in Spam is asking for spam,
 * so the token is written for you.
 *
 * Written into the field rather than applied behind it, so it reads as part of
 * the query, lights the chip that matches it, and can be deleted to widen —
 * the same rule every other filter here follows. Applied only as a search
 * begins, never on each keystroke, so deleting the token does not fight you.
 */
export function scopedQuery(next: string, previous: string, view: string): string {
  const starting = !previous.trim() && next.trim().length > 0;
  const binned = view === 'spam' || view === 'trash';
  if (!starting || !binned) return next;
  return `in:${view} ${next}`;
}
