import { folderDelimiter, folderLeaf } from './folders';
import { t, type StringId } from './strings';

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

/**
 * The field, read the way the engine reads it (`search_query.rs`) and kept
 * the way it was typed.
 *
 * Both halves matter. Chips compare against what a piece *says* — its text
 * with the quote marks off — so `in:"Client contact"` in the field lights the
 * chip that writes the same thing. Whatever goes back into the field goes
 * back as the chunk it came from, untouched: the grammar has phrases,
 * exclusion, brackets and `AND` `OR` `NOT`, and rebuilding a query from its
 * bare pieces would turn `"OR"` into the operator, `"from:sam"` into a
 * filter, and `-"board pack"` into the phrase `"-board pack"` — which asks
 * for exactly what it was excluding.
 */
export type Lexeme = {
  kind: 'open' | 'close' | 'minus' | 'piece';
  /** Which whitespace-separated chunk of the field this came from. */
  chunk: number;
  /** How many brackets were open when it was read. */
  depth: number;
  /** A piece's text with its quote marks off and its spacing evened out. */
  says: string;
  /** No quote mark anywhere in it. */
  plain: boolean;
  /** Opened with a quote, so it is words to look for and never an operator,
   *  whatever it spells. */
  phrase: boolean;
  /** Led by a `-` that is outside any quotes: what it says is excluded. */
  negated: boolean;
};

const QUOTES = '"“”„';
const KEYWORDS = ['AND', 'OR', 'NOT'];

export function read(query: string): { chunks: string[]; lexemes: Lexeme[] } {
  const chunks: string[] = [];
  const lexemes: Lexeme[] = [];
  let raw = '';
  let says = '';
  let quoteAt: number | null = null;
  let quoted = false;
  let depth = 0;
  const push = (kind: Lexeme['kind']) => {
    const before = depth;
    if (kind === 'open') depth += 1;
    if (kind === 'close') depth = Math.max(0, depth - 1);
    lexemes.push({
      kind,
      chunk: chunks.length,
      depth: before,
      says: '',
      plain: true,
      phrase: false,
      negated: false,
    });
  };
  const flushPiece = () => {
    if (says) {
      const negated = says.length > 1 && says.startsWith('-') && quoteAt !== 0;
      lexemes.push({
        kind: 'piece',
        chunk: chunks.length,
        depth,
        says,
        plain: quoteAt === null,
        phrase: quoteAt === (negated ? 1 : 0),
        negated,
      });
    }
    says = '';
    quoteAt = null;
  };
  const flushChunk = () => {
    flushPiece();
    if (raw) chunks.push(raw);
    raw = '';
  };
  const chars = [...query];
  chars.forEach((ch, i) => {
    const bare = !quoted && quoteAt === null;
    const next = chars[i + 1];
    if (QUOTES.includes(ch)) {
      quoted = !quoted;
      quoteAt ??= says.length;
      raw += ch;
    } else if (/\s/.test(ch)) {
      if (!quoted) flushChunk();
      else {
        raw += ch;
        if (!says.endsWith(' ')) says += ' ';
      }
    } else if (ch === '(' && bare && (says === '' || says === '-' || KEYWORDS.includes(says))) {
      // A bracket groups only where it could not be part of a word.
      if (says === '-') {
        says = '';
        push('minus');
      } else flushPiece();
      push('open');
      raw += ch;
    } else if (ch === ')' && !quoted && (next === undefined || next === ')' || /\s/.test(next))) {
      flushPiece();
      push('close');
      raw += ch;
    } else {
      raw += ch;
      says += ch;
    }
  });
  flushChunk();
  return { chunks, lexemes };
}

/** Splits a query the way the engine does, keeping `from:"Dana Wu"` whole. */
export function tokensOf(query: string): string[] {
  return read(query)
    .lexemes.filter((l) => l.kind === 'piece')
    .map((l) => l.says);
}

export const isKeyword = (l: Lexeme) =>
  l.kind === 'piece' && l.plain && KEYWORDS.includes(l.says);

/** What `OR` separates outside every bracket, each a run of lexemes. */
function alternativesOf(lexemes: Lexeme[]): Lexeme[][] {
  const groups: Lexeme[][] = [[]];
  for (const l of lexemes) {
    if (l.depth === 0 && isKeyword(l) && l.says === 'OR') groups.push([]);
    else groups[groups.length - 1].push(l);
  }
  return groups;
}

/** Whether a lexeme is this operator, standing outside every bracket — as
 *  opposed to a phrase that spells it, or one side of a bracketed choice. */
const is = (l: Lexeme, want: string) =>
  l.kind === 'piece' && l.depth === 0 && !l.phrase && l.says.toLowerCase() === want;

/** Whether the query already carries this operator.
 *
 *  Both sides are read the way the engine reads them, so a chip written
 *  `in:"Client contact"` matches the same token in the field. Comparing the
 *  raw strings meant a chip with a space in its value never lit, never
 *  sorted with the applied ones, and could not be clicked off — clicking it
 *  added a second copy.
 *
 *  `OR` binds looser than everything else, so a filter narrows the whole
 *  result only when every alternative carries it. `from:sam is:unread OR
 *  from:dana` is not an unread search, and the chip must not say it is. */
export function hasToken(query: string, token: string): boolean {
  const want = tokensOf(token)[0]?.toLowerCase();
  if (want === undefined) return false;
  const { lexemes } = read(query);
  const groups = alternativesOf(lexemes).filter(
    (g) => g.length > 0,
  );
  return groups.length > 0 && groups.every((g) => g.some((l) => is(l, want)));
}

/**
 * The operators a chip sets rather than adds.
 *
 * Two `from:` values both have to hold, and so do two `in:`: sam-and-dana is
 * nobody, and a message is rarely in two places. A chip for one of these
 * means "this sender", "this mailbox", "this year", so it replaces what is
 * there instead of narrowing to nothing.
 *
 * `is:` and `has:` are the other kind. Each is an independent condition —
 * unread, starred, snoozed, has an attachment — and they narrow together.
 * Replacing one with another is how clicking Unread in the Snoozed view threw
 * the view away and searched the whole mailbox instead.
 */
const SINGLE_VALUE = ['from:', 'in:', 'after:'];

/** The query with its outermost brackets off, when they hold all of it. */
function unwrapped(query: string): string {
  const { chunks, lexemes } = read(query);
  const first = lexemes[0];
  const last = lexemes[lexemes.length - 1];
  const whole =
    first?.kind === 'open' &&
    last?.kind === 'close' &&
    lexemes.slice(1, -1).every((l) => l.depth >= 1) &&
    chunks[0].startsWith('(') &&
    chunks[chunks.length - 1].endsWith(')');
  if (!whole) return query;
  const inner = [...chunks];
  inner[0] = inner[0].slice(1);
  inner[inner.length - 1] = inner[inner.length - 1].slice(0, -1);
  return inner.filter(Boolean).join(' ');
}

/** Adds the token, or takes it away if it is already there.
 *
 *  A filter narrows the whole result. `OR` binds loosest, so a token written
 *  after one would hold for the last alternative alone while the chip lit up
 *  as though it held for all: the query goes into brackets first, and comes
 *  out of them again when the chip comes off. */
export function toggleToken(query: string, token: string): string {
  const want = tokensOf(token)[0]?.toLowerCase();
  if (want === undefined) return query;
  const key = want.includes(':') ? `${want.split(':')[0]}:` : null;
  const { chunks, lexemes } = read(query);
  const groups = alternativesOf(lexemes).filter(
    (g) => g.length > 0,
  );
  const without = (dropped: (l: Lexeme) => boolean) => {
    const gone = new Set(lexemes.filter(dropped).map((l) => l.chunk));
    return chunks.filter((_, i) => !gone.has(i));
  };

  if (hasToken(query, token)) {
    return unwrapped(without((l) => is(l, want)).join(' '));
  }
  if (groups.length === 0) return token;
  if (groups.length > 1) return `(${chunks.join(' ')}) ${token}`;

  // One alternative, or none yet. The opposite of what is being asked for
  // cannot stay beside it, and neither can another value for an operator
  // that takes one.
  const kept = without(
    (l) =>
      is(l, `-${want}`) ||
      Boolean(
        key &&
          SINGLE_VALUE.includes(key) &&
          l.kind === 'piece' &&
          l.depth === 0 &&
          !l.phrase &&
          l.says.toLowerCase().startsWith(key),
      ),
  );
  // An `OR` left dangling at the end is somebody mid-way through typing the
  // next alternative; the token belongs before it, with what it narrows.
  const dangling = groups.length === 1 && kept[kept.length - 1] === 'OR';
  return (dangling ? [...kept.slice(0, -1), token, 'OR'] : [...kept, token]).join(' ');
}

/** The first `in:` or `from:` the query applies, as the bare token — not one
 *  it excludes, not a phrase that happens to spell one, and not one side of
 *  a bracketed choice. */
function appliedValue(query: string, key: string): string | undefined {
  return read(query).lexemes.find(
    (l) =>
      l.kind === 'piece' &&
      l.depth === 0 &&
      !l.phrase &&
      l.says.length > key.length &&
      l.says.toLowerCase().startsWith(key),
  )?.says;
}

/**
 * The query with a run of its words as one phrase, when it looks as though
 * that is what was meant — or null when nothing suggests it.
 *
 * `AND`, `OR` and `NOT` in capitals are operators, always; the engine does
 * not guess. But `TERMS AND CONDITIONS` and `DO NOT REPLY` are subject lines
 * somebody pasted, and read as Boolean the second finds exactly the mail that
 * lacks "reply". Nothing in the text can prove which was meant, so nothing is
 * decided here either: the field offers the other reading as a chip, and the
 * one person who knows takes it or does not.
 *
 * The sign it goes by is that the capitals do not stand out. In `invoice NOT
 * draft` the keyword is the only thing shouting, which is how an operator is
 * typed. Beside another word in capitals it is just one more of them.
 *
 * What is offered is the phrase, not the keyword alone in quotes. Somebody
 * who pasted a subject line is looking for that line: `"TERMS AND
 * CONDITIONS"`, those words in that order — which is also the more useful
 * search, since `TERMS "AND" CONDITIONS` finds any mail with the three words
 * anywhere in it. The phrase is the whole run of ordinary words the keyword
 * sits in; operators and brackets end it, so `in:inbox` stays a scope.
 */
export function asWords(query: string): { words: string; rewrite: string } | null {
  const { chunks, lexemes } = read(query);
  // An ordinary word or a keyword, standing alone in its chunk: no operator,
  // no quotes, no exclusion, no bracket attached.
  const word = (l: Lexeme | undefined): l is Lexeme =>
    l?.kind === 'piece' &&
    l.plain &&
    !l.says.includes(':') &&
    !l.says.startsWith('-') &&
    chunks[l.chunk] === l.says;
  const capitals = (l: Lexeme | undefined) =>
    word(l) && !isKeyword(l) && l.says !== l.says.toLowerCase() && l.says === l.says.toUpperCase();
  const doubtful = lexemes
    .map((l, at) => at)
    .filter(
      (at) =>
        isKeyword(lexemes[at]) &&
        word(lexemes[at]) &&
        (capitals(lexemes[at - 1]) || capitals(lexemes[at + 1])),
    );
  if (doubtful.length === 0) return null;

  // Each doubtful keyword, widened to the run of words around it.
  const phrases = new Map<number, number>();
  for (const at of doubtful) {
    let first = at;
    let last = at;
    while (word(lexemes[first - 1]) && lexemes[first - 1].depth === lexemes[at].depth) first -= 1;
    while (word(lexemes[last + 1]) && lexemes[last + 1].depth === lexemes[at].depth) last += 1;
    phrases.set(lexemes[first].chunk, lexemes[last].chunk);
  }
  const out: string[] = [];
  let shown = '';
  for (let i = 0; i < chunks.length; i += 1) {
    const end = phrases.get(i);
    if (end === undefined) out.push(chunks[i]);
    else {
      const phrase = chunks.slice(i, end + 1).join(' ');
      shown ||= phrase;
      out.push(`"${phrase}"`);
      i = end;
    }
  }
  return { words: shown, rewrite: out.join(' ') };
}

/** The chips on offer, in the order the mockup shows them. */
/** `rewrite` is for the one kind of chip that is not a filter: it swaps the
 *  whole query for another reading of it rather than toggling a token. */
export type Chip = { id: string; label: string; token: string; rewrite?: string };

/**
 * The mailboxes a scope chip can name.
 *
 * `in:` resolves against folder roles and — since folders the user made
 * became searchable — against a folder's own name. Starred is a flag and
 * tags are a table of their own; neither gets a scope chip rather than
 * getting one that silently matches nothing.
 */
const SCOPES: Record<string, StringId> = {
  inbox: 'mailbox-inbox',
  archive: 'mailbox-archive',
  sent: 'mailbox-sent',
  drafts: 'mailbox-drafts',
  spam: 'mailbox-spam',
  trash: 'mailbox-trash',
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
  if (role) return { token: `in:${view}`, label: t('search-chip-in', { where: t(role) }) };
  // Starred and Snoozed are states, not places — their scope speaks `is:`.
  if (view === 'starred') return { token: 'is:starred', label: t('search-chip-starred') };
  if (view === 'snoozed') return { token: 'is:snoozed', label: t('search-chip-snoozed') };
  if (view.startsWith('folder:') && leaf) {
    const value = /\s/.test(leaf) ? `"${leaf}"` : leaf;
    return { token: `in:${value}`, label: t('search-chip-in', { where: leaf }) };
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
  const appliedIn = appliedValue(query, 'in:');
  const sameAsContext =
    appliedIn !== undefined &&
    tokensOf(context?.token ?? '')[0]?.toLowerCase() === appliedIn.toLowerCase();
  const scope =
    appliedIn && !sameAsContext
      ? { token: quoted(appliedIn), label: t('search-chip-in', { where: appliedIn.slice('in:'.length) }) }
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
  const inQuery = appliedValue(query, 'from:');
  const who = inQuery ? inQuery.slice('from:'.length) : sender;
  if (who) {
    list.push({ id: 'from', label: t('search-chip-from', { who }), token: quoted(`from:${who}`) });
  }
  list.push(
    { id: 'attachment', label: t('search-chip-attachment'), token: 'has:attachment' },
    { id: 'unread', label: t('search-chip-unread'), token: 'is:unread' },
  );
  // Not doubled when the scope already is it.
  if (scope?.token !== 'is:starred') {
    list.push({ id: 'starred', label: t('search-chip-starred'), token: 'is:starred' });
  }
  list.push({ id: 'year', label: t('search-chip-year'), token: `after:${year}` });
  // Applied first, each group still in the order above. Two filters rather
  // than a sort, because a sort is only stable by promise and this ordering
  // is the whole point: the row must not shuffle within a group as tokens
  // come and go.
  const applied = (c: Chip) => hasToken(query, c.token);
  const other = asWords(query);
  const suggestion: Chip[] = other
    ? [
        {
          id: 'as-words',
          label: t('search-chip-as-words', { words: other.words }),
          token: '',
          rewrite: other.rewrite,
        },
      ]
    : [];
  return [...list.filter(applied), ...list.filter((c) => !applied(c)), ...suggestion];
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
