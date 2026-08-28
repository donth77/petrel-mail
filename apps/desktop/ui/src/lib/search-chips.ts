import { folderDelimiter, folderLeaf } from './folders';

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

/** A value with whitespace in it, put back the way it was typed.
 *
 *  `tokensOf` strips the quotes so the value can be read; anything that goes
 *  back into the field has to wear them again, or `in:"Client contact"`
 *  returns as two words and the query quietly means something else. */
export function quoted(token: string): string {
  const at = token.indexOf(':');
  if (at === -1) return /\s/.test(token) ? `"${token}"` : token;
  const value = token.slice(at + 1);
  return /\s/.test(value) ? `${token.slice(0, at)}:"${value}"` : token;
}

/** Whether the query already carries this operator.
 *
 *  Both sides are read the way the engine reads them, so a chip written
 *  `in:"Client contact"` matches the same token in the field. Comparing the
 *  raw strings meant a chip with a space in its value never lit, never
 *  sorted with the applied ones, and could not be clicked off — clicking it
 *  added a second copy. */
export function hasToken(query: string, token: string): boolean {
  const want = tokensOf(token)[0]?.toLowerCase();
  return want !== undefined && tokensOf(query).some((t) => t.toLowerCase() === want);
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
 * The operators the engine reads as a single value.
 *
 * `from:`, `in:` and `after:` each land in one field of the parsed query, so a
 * second token silently overwrites the first: two senders is a query that
 * matches nothing, and nobody means it. A chip for one of these replaces what
 * is there.
 *
 * `is:` and `has:` are the other kind. Each sets an independent condition —
 * unread, starred, snoozed, has an attachment — and they narrow together.
 * Replacing one with another is how clicking Unread in the Snoozed view threw
 * the view away and searched the whole mailbox instead.
 */
const SINGLE_VALUE = ['from:', 'in:', 'after:'];

/** Adds the token, or takes it away if it is already there. */
export function toggleToken(query: string, token: string): string {
  const key = token.includes(':') ? `${token.split(':')[0].toLowerCase()}:` : null;
  const tokens = tokensOf(query);
  // Every path below rebuilds the query from its tokens, and `tokensOf` has
  // taken the quotes off. They go back on, or toggling Unread while standing
  // in `Client contact` rewrote the scope as two bare words.
  const rebuild = (parts: string[]) => parts.map(quoted).join(' ').trim();

  const want = tokensOf(token)[0]?.toLowerCase();
  if (hasToken(query, token)) {
    return rebuild(tokens.filter((t) => t.toLowerCase() !== want));
  }

  const kept =
    key && SINGLE_VALUE.includes(key)
      ? tokens.filter((t) => !t.toLowerCase().startsWith(key))
      : tokens;
  return rebuild([...kept, ...tokensOf(token)]);
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
 *  cannot name its folder is better absent than wrong.
 *
 *  The leaf comes from the same reading of the hierarchy the rest of the app
 *  uses, so a folder honestly named `example.com` is not searched for as
 *  `com`. */
export function folderScopeName(
  view: string,
  folders: ReadonlyArray<{ id: number; path: string }>,
): string | null {
  if (!view.startsWith('folder:')) return null;
  const f = folders.find((x) => `folder:${x.id}` === view);
  return f ? folderLeaf(f.path, folderDelimiter(folders)) : null;
}

/** What the open view is called in the search grammar, or null when the
 *  grammar cannot say it (a tag view, the outbox). For a user folder the
 *  caller supplies the folder's leaf name, because folders live with it. */
export function scopeFor(
  view: string,
  leaf?: string | null,
): { token: string; label: string } | null {
  const role = SCOPES[view];
  if (role) return { token: `in:${view}`, label: `In ${role}` };
  // Starred and Snoozed are states, not places — their scope speaks `is:`.
  if (view === 'starred') return { token: 'is:starred', label: 'Starred' };
  if (view === 'snoozed') return { token: 'is:snoozed', label: 'Snoozed' };
  if (view.startsWith('folder:') && leaf) {
    const value = /\s/.test(leaf) ? `"${leaf}"` : leaf;
    return { token: `in:${value}`, label: `In ${leaf}` };
  }
  return null;
}

/**
 * @param view The mailbox on screen. Its scope chip is built *first*: a search
 *   typed here starts scoped to where you are standing (the token is written
 *   into the field, visible and deletable), so the chip that mirrors that
 *   context leads the row. Deleting the token widens to everything — and the
 *   command palette searches globally from the start.
 * @param query What is in the field, which decides the order the row comes
 *   back in: everything applied first, then everything on offer. A row that
 *   reads lit, unlit, lit makes the reader scan it to answer "what am I
 *   filtering by"; grouped, the answer is the first run of chips. Usually
 *   this changes nothing, because the scope is the applied one and it leads
 *   anyway.
 */
export function chips(
  sender: string | null,
  year: number,
  view: string,
  leaf?: string | null,
  query = '',
): Chip[] {
  const list: Chip[] = [];
  // First, and named for the mailbox actually being searched.
  //
  // Ordinarily that is where you are standing, which is what the pre-applied
  // scope token says. But a query can name somewhere else — typed by hand, or
  // carried over — and the row then offered the open mailbox, unlit, while
  // the real scope narrowed the list with no pill at all. The applied filter
  // wins; the open mailbox is only the offer when nothing else is scoping.
  const context = scopeFor(view, leaf);
  const appliedIn = tokensOf(query).find((t) => t.toLowerCase().startsWith('in:'));
  const sameAsContext =
    appliedIn !== undefined &&
    tokensOf(context?.token ?? '')[0]?.toLowerCase() === appliedIn.toLowerCase();
  const scope =
    appliedIn && !sameAsContext
      ? { token: quoted(appliedIn), label: `In ${appliedIn.slice('in:'.length)}` }
      : context;
  if (scope) list.push({ id: 'scope', label: scope.label, token: scope.token });
  // The sender of whatever is open, because "more from this person" is the
  // search people actually run — and it is tedious to type.
  //
  // A `from:` already in the field wins over it. The chip used to be built
  // only from the open conversation, so running the search emptied the
  // selection and took the pill away while `from:Slack` went on filtering the
  // list: a filter with no way to see it and no way to switch it off. Opening
  // a different message did the same thing more quietly, by relabelling the
  // pill after somebody else.
  const inQuery = tokensOf(query).find((t) => t.toLowerCase().startsWith('from:'));
  const who = inQuery ? inQuery.slice('from:'.length) : sender;
  if (who) {
    list.push({ id: 'from', label: `From ${who}`, token: quoted(`from:${who}`) });
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
  // Applied first, each group still in the order above. Two filters rather
  // than a sort, because a sort is only stable by promise and this ordering
  // is the whole point: the row must not shuffle within a group as tokens
  // come and go.
  const applied = (c: Chip) => hasToken(query, c.token);
  return [...list.filter(applied), ...list.filter((c) => !applied(c))];
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
