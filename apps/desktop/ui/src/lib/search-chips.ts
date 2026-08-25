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
 * `in:` resolves against folder roles and — since folders the user made
 * became searchable — against a folder's own name. Starred is a flag and
 * tags are a table of their own; neither gets a scope chip rather than
 * getting one that silently matches nothing.
 */
const SCOPES: Record<string, string> = {
  inbox: 'Inbox',
  archive: 'Archive',
  sent: 'Sent',
  drafts: 'Drafts',
  spam: 'Spam',
  trash: 'Trash',
};

/** The leaf name of the open folder view — what a search scope calls it.
 *  Null when the view is not a folder, or the folder is not (yet) known:
 *  reference data loads a beat after the view can change, and a scope that
 *  cannot name its folder is better absent than wrong. */
export function folderLeaf(
  view: string,
  folders: ReadonlyArray<{ id: number; path: string }>,
): string | null {
  if (!view.startsWith('folder:')) return null;
  const f = folders.find((x) => `folder:${x.id}` === view);
  return f?.path.split(/[/.]/).pop() ?? null;
}

/** What the open view is called in the search grammar, or null when the
 *  grammar cannot say it (a tag view, the outbox). For a user folder the
 *  caller supplies the folder's leaf name, because folders live with it. */
export function scopeFor(
  view: string,
  folderLeaf?: string | null,
): { token: string; label: string } | null {
  const role = SCOPES[view];
  if (role) return { token: `in:${view}`, label: `In ${role}` };
  // Starred and Snoozed are states, not places — their scope speaks `is:`.
  if (view === 'starred') return { token: 'is:starred', label: 'Starred' };
  if (view === 'snoozed') return { token: 'is:snoozed', label: 'Snoozed' };
  if (view.startsWith('folder:') && folderLeaf) {
    const value = /\s/.test(folderLeaf) ? `"${folderLeaf}"` : folderLeaf;
    return { token: `in:${value}`, label: `In ${folderLeaf}` };
  }
  return null;
}

/**
 * @param view The mailbox on screen. Its scope chip comes *first*: a search
 *   typed here starts scoped to where you are standing (the token is written
 *   into the field, visible and deletable), so the chip that mirrors that
 *   context leads the row. Deleting the token widens to everything — and the
 *   command palette searches globally from the start.
 */
export function chips(
  sender: string | null,
  year: number,
  view: string,
  folderLeaf?: string | null,
): Chip[] {
  const list: Chip[] = [];
  // First, and named for where you actually are — the pre-applied context.
  const scope = scopeFor(view, folderLeaf);
  if (scope) list.push({ id: 'scope', label: scope.label, token: scope.token });
  // The sender of whatever is open, because "more from this person" is the
  // search people actually run — and it is tedious to type.
  if (sender) {
    const value = /\s/.test(sender) ? `"${sender}"` : sender;
    list.push({ id: 'from', label: `From ${sender}`, token: `from:${value}` });
  }
  list.push(
    { id: 'attachment', label: 'Has attachment', token: 'has:attachment' },
    { id: 'unread', label: 'Unread', token: 'is:unread' },
  );
  // Not doubled when the scope already is it.
  if (scope?.token !== 'is:starred') {
    list.push({ id: 'starred', label: 'Starred', token: 'is:starred' });
  }
  list.push({ id: 'year', label: 'This year', token: `after:${year}` });
  return list;
}

/**
 * A search starts where you are standing.
 *
 * As the first character lands, the open view's scope is written into the
 * field — `in:inbox`, `in:sent`, `in:Receipts` — so the top bar answers for
 * the context on screen, the way a person expects a search box above a list
 * to behave. Written into the field rather than applied behind it, so it
 * reads as part of the query, lights the leading chip, and can be deleted to
 * widen to everything; the command palette searches globally from the start.
 * Applied only as a search begins, never on each keystroke, so deleting the
 * token does not fight you. Spam and Trash still ride this rule — standing
 * in them is the asking that lets a search see them at all.
 */
export function scopedQuery(next: string, previous: string, scopeToken: string | null): string {
  const starting = !previous.trim() && next.trim().length > 0;
  if (!starting || !scopeToken) return next;
  return `${scopeToken} ${next}`;
}
