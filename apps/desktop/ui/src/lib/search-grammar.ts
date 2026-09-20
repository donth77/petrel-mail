/**
 * The search field's grammar, as the field has to read it.
 *
 * The engine's parser is `crates/petrel-engine/src/search_query.rs`, and it
 * is the one that decides what a query means. This is the same reading, on
 * this side of the wall, for the three things the window has to do before the
 * engine ever sees the text: light the chips that match what was typed, mark
 * the words a result was found by, and say when a query was cut short.
 *
 * Kept the way it was typed as well as the way it reads. The grammar has
 * phrases, exclusion, brackets and `AND` `OR` `NOT`, and rebuilding a query
 * from its bare pieces would turn `"OR"` into the operator, `"from:sam"` into
 * a filter, and `-"board pack"` into the phrase `"-board pack"` — which asks
 * for exactly what it was excluding.
 */

/** One piece of the field, as it reads and as it was typed. Chips compare
 *  against what a piece *says* — its text with the quote marks off — so
 *  `in:"Client contact"` in the field lights the chip that writes the same
 *  thing, while what goes back into the field is the chunk it came from. */
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
  /** Where in `says` the first quote mark stood, or null. A colon before it
   *  can make an operator (`from:"Dana Wu"`); one after it cannot. */
  quoteAt: number | null;
  /** Opened with a quote, so it is words to look for and never an operator,
   *  whatever it spells. */
  phrase: boolean;
  /** Led by a `-` that is outside any quotes: what it says is excluded. */
  negated: boolean;
  /** The chunks of the `NOT`s standing directly in front of a piece. An odd
   *  number of them excludes it, exactly as a `-` would, and they go wherever
   *  the piece goes: taken out without them, a `NOT` would be left to fall on
   *  whatever came next. */
  nots: number[];
};

const QUOTES = '"“”„';
/** The words that are operators in capitals and words otherwise. */
export const KEYWORDS = ['AND', 'OR', 'NOT'];
/** Whitespace, and the control characters the engine reads as the same. */
// eslint-disable-next-line no-control-regex
const SPACING = /[\s\u0000-\u001f\u007f-\u009f]/;

export const isKeyword = (l: Lexeme) =>
  l.kind === 'piece' && l.plain && KEYWORDS.includes(l.says);

export function read(query: string): {
  chunks: string[];
  lexemes: Lexeme[];
  /** What it would take to close the quote and the brackets still open at
   *  the end, which is what a query looks like while it is being typed. */
  unclosed: string;
} {
  const chunks: string[] = [];
  const lexemes: Lexeme[] = [];
  let raw = '';
  let says = '';
  let quoteAt: number | null = null;
  let quoted = false;
  let openedWith = '"';
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
      quoteAt: null,
      phrase: false,
      negated: false,
      nots: [],
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
        quoteAt,
        phrase: quoteAt === (negated ? 1 : 0),
        negated,
        nots: [],
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
      if (quoted) openedWith = ch;
      quoteAt ??= says.length;
      raw += ch;
    } else if (SPACING.test(ch)) {
      if (!quoted) flushChunk();
      else {
        raw += ch;
        if (!says.endsWith(' ')) says += ' ';
      }
    } else if (ch === '(' && bare && (/^-*$/.test(says) || KEYWORDS.includes(says))) {
      // A bracket groups only where it could not be part of a word. However
      // many dashes stand in front of it, they are one exclusion.
      if (says !== '' && !KEYWORDS.includes(says)) {
        says = '';
        push('minus');
      } else flushPiece();
      push('open');
      raw += ch;
    } else if (ch === ')' && !quoted && (next === undefined || next === ')' || SPACING.test(next))) {
      flushPiece();
      push('close');
      raw += ch;
    } else {
      raw += ch;
      says += ch;
    }
  });
  flushChunk();

  let run: number[] = [];
  for (const l of lexemes) {
    if (isKeyword(l) && l.says === 'NOT') run.push(l.chunk);
    else {
      if (l.kind === 'piece' && !isKeyword(l)) l.nots = run;
      run = [];
    }
  }
  const closer = openedWith === '“' || openedWith === '„' ? '”' : '"';
  return { chunks, lexemes, unclosed: (quoted ? closer : '') + ')'.repeat(depth) };
}

/** Splits a query the way the engine does, keeping `from:"Dana Wu"` whole. */
export function tokensOf(query: string): string[] {
  return read(query)
    .lexemes.filter((l) => l.kind === 'piece')
    .map((l) => l.says);
}

/** The operators the engine knows. Anything else before a colon is text. */
const OPERATORS = [
  'from', 'to', 'cc', 'subject', 'in', 'tag', 'filename', 'is', 'has', 'after', 'before', 'date',
];

/** Whether the engine reads `key:value` as the operator. One it does not
 *  recognise, or a value it cannot take, is searched for as words
 *  (`search_query.rs`). */
function operates(key: string, value: string): boolean {
  if (!OPERATORS.includes(key) || !value) return false;
  const low = value.toLowerCase();
  if (key === 'is') return ['unread', 'read', 'starred', 'flagged', 'snoozed'].includes(low);
  if (key === 'has') return ['attachment', 'attachments', 'file'].includes(low);
  if (key === 'after' || key === 'before' || key === 'date') {
    const parts = /^(\d{4})(?:[-/](\d{1,2})(?:[-/](\d{1,2}))?)?[-/]?$/.exec(value);
    if (!parts) return false;
    const [year, month, day] = [parts[1], parts[2], parts[3]].map((n) =>
      n === undefined ? 1 : Number(n),
    );
    // The same days the engine's `Period` takes, so a date it reads as
    // words is read as words here: 9998 so that the day after a period is
    // still a date, and February knows which years are leap years.
    const leap = (year % 4 === 0 && year % 100 !== 0) || year % 400 === 0;
    const days = [31, leap ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if (year < 1970 || year > 9998 || month < 1 || month > 12) return false;
    return day >= 1 && day <= days[month - 1];
  }
  return true;
}

/** A piece as the engine's `read` takes it apart. */
export type Reading = {
  /** Excluded by its own leading dashes. */
  negated: boolean;
  /** What it says with those dashes off. */
  text: string;
  /** The operator, lowercased, when the engine applies one; else null. */
  key: string | null;
  /** The operator's value, or else the text, trimmed: what the engine keeps. */
  value: string;
};

/**
 * One piece, read the way `search_query.rs` reads it: the dashes in front of
 * it are one exclusion unless they are all there is, and a colon makes an
 * operator only when it stands outside the quotes and the engine knows the
 * word before it and can take the value after it.
 */
export function reading(l: Lexeme): Reading {
  const upTo = l.quoteAt ?? l.says.length;
  const dashes = l.says.slice(0, upTo).length - l.says.slice(0, upTo).replace(/^-+/, '').length;
  const negated = dashes > 0 && dashes < l.says.length;
  const text = negated ? l.says.slice(dashes) : l.says;
  const quoteAt = l.quoteAt === null ? null : l.quoteAt - (negated ? dashes : 0);
  const colon = text.indexOf(':');
  if (colon >= 0 && (quoteAt === null || colon < quoteAt)) {
    const key = text.slice(0, colon).toLowerCase();
    const value = text.slice(colon + 1).trim();
    // A mailbox is held lowercased, as the engine holds it: the roles are
    // lowercase and the store asks for them by name.
    if (operates(key, value)) {
      return { negated, text, key, value: key === 'in' ? value.toLowerCase() : value };
    }
  }
  return { negated, text, key: null, value: text.trim() };
}
