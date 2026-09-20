import { describe, expect, it } from 'vitest';
import { isKeyword, read, reading, tokensOf } from './search-grammar';
import { cutShort } from './search-limits';
import cases from './search-grammar.cases.json?raw';

/** One piece, taken apart the way the engine takes it apart. */
const apart = (query: string) => {
  const piece = read(query).lexemes.find((l) => l.kind === 'piece');
  if (!piece) throw new Error(`no piece in ${query}`);
  const r = reading(piece);
  return `${r.negated ? '-' : ''}${r.key === null ? '' : `${r.key}:`}${r.value}`;
};

/* The one place the field's reading of a piece is written down, used by the
   highlighter and by the line that says when a query was cut short. The
   engine's own rules are in `read` in search_query.rs. */

describe('a piece, read the way the engine reads it', () => {
  it('takes the operator only where the engine would', () => {
    expect(apart('from:sam')).toBe('from:sam');
    expect(apart('FROM:Sam')).toBe('from:Sam');
    expect(apart('from:"Dana Wu"')).toBe('from:Dana Wu');
    // Not an operator it knows, or not a value it can take: plain words.
    expect(apart('re:pricing')).toBe('re:pricing');
    expect(apart('is:whatever')).toBe('is:whatever');
    expect(apart('after:1969')).toBe('after:1969');
    expect(apart('after:2026-02-30')).toBe('after:2026-02-30');
    expect(apart('after:2028-02-29')).toBe('after:2028-02-29');
    // A quote before the colon makes it words, wherever the quote opens.
    expect(apart('"from:sam"')).toBe('from:sam');
    expect(apart('fr"om:sam"')).toBe('from:sam');
  });

  it('reads the dashes in front of it as one exclusion', () => {
    expect(apart('-draft')).toBe('-draft');
    expect(apart('--draft')).toBe('-draft');
    expect(apart('-from:sam')).toBe('-from:sam');
    expect(apart('-"board pack"')).toBe('-board pack');
    // A dash with nothing after it is a dash.
    expect(apart('-')).toBe('-');
    expect(apart('e-mail')).toBe('e-mail');
  });

  it('trims the value the way the engine keeps it', () => {
    expect(apart('"  board pack  "')).toBe('board pack');
    expect(apart('from:"  sam  "')).toBe('from:sam');
  });
});

describe('tokensOf', () => {
  it('keeps a quoted value whole', () => {
    expect(tokensOf('from:"Dana Wu" annex')).toEqual(['from:Dana Wu', 'annex']);
  });
});

/* The cases the field and the engine share. The engine's side of them is
   `apps/desktop/src-tauri/tests/search_field.rs`, so a rule that changes on
   one side and not the other fails on both. */
describe('what the engine will make of a query', () => {
  const shared = JSON.parse(cases) as {
    cases: { query: string; reads: string[]; cut: string | null }[];
    long: {
      cases: { prefix: string; suffix?: string; value: string; times: number; cut: string | null }[];
    };
    many: { cases: { words: number; cut: string | null }[] }; 
  };

  /** Every term of a query, in the order the field reads them. */
  const reads = (query: string) =>
    read(query)
      .lexemes.filter((l) => l.kind === 'piece' && !isKeyword(l))
      .map((l) => reading(l))
      .filter((r) => r.key !== null || r.value !== '')
      .map((r) => `${r.negated ? '-' : ''}${r.key ?? 'text'}:${r.value}`);

  it.each(shared.cases)('$query', ({ query, reads: expected, cut }) => {
    expect(reads(query)).toEqual(expected);
    expect(cutShort(query)).toBe(cut);
  });

  it('agrees about a term that is too long', () => {
    for (const { prefix, suffix = '', value, times, cut } of shared.long.cases) {
      expect(cutShort(prefix + value.repeat(times) + suffix), `${prefix}${value}×${times}`).toBe(
        cut,
      );
    }
  });

  it('agrees about a query with too many terms', () => {
    for (const { words, cut } of shared.many.cases) {
      const query = Array.from({ length: words }, (_, i) => `w${i}`).join(' ');
      expect(cutShort(query)).toBe(cut);
    }
  });
});
