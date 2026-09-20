import { describe, expect, it } from 'vitest';
import { MAX_CLAUSES, MAX_VALUE_CHARS, cutShort } from './search-limits';

/** `n` distinct words. */
const words = (n: number) => Array.from({ length: n }, (_, i) => `w${i}`).join(' ');

/* These are the cases `search_query.rs` asserts `truncated` for, read the
   same way here. When one side changes, the other has to. */
describe('when a query is cut short', () => {
  it('is not, up to the limits', () => {
    expect(cutShort('')).toBeNull();
    expect(cutShort('annex pricing')).toBeNull();
    expect(cutShort(words(MAX_CLAUSES))).toBeNull();
    expect(cutShort(`from:${'x'.repeat(MAX_VALUE_CHARS)}`)).toBeNull();
    expect(cutShort('x'.repeat(MAX_VALUE_CHARS))).toBeNull();
  });

  it('is past the number of terms', () => {
    expect(cutShort(words(MAX_CLAUSES + 1))).toBe('terms');
    expect(cutShort(words(MAX_CLAUSES + 5))).toBe('terms');
  });

  it('counts terms the way the engine does', () => {
    // AND, OR, NOT and brackets are not terms…
    const joined = Array.from({ length: MAX_CLAUSES }, (_, i) => `w${i}`).join(' OR ');
    expect(cutShort(`(${joined}) AND NOT`)).toBeNull();
    expect(cutShort(`NOT (${joined})`)).toBeNull();
    // …and neither is an empty pair of quotes, or one holding only spaces.
    expect(cutShort(`${words(MAX_CLAUSES)} "" " "`)).toBeNull();
    // A filter is one, and so is a lowercase `or`, which is a word.
    expect(cutShort(`${words(MAX_CLAUSES)} is:unread`)).toBe('terms');
    expect(cutShort(`${words(MAX_CLAUSES)} or`)).toBe('terms');
    // A quoted phrase is one term however many words are in it.
    expect(cutShort(`${words(MAX_CLAUSES - 1)} "${words(20)}"`)).toBeNull();
  });

  it('is past the length of one term', () => {
    const long = 'x'.repeat(MAX_VALUE_CHARS + 1);
    expect(cutShort(long)).toBe('value');
    expect(cutShort(`from:${long}`)).toBe('value');
    expect(cutShort(`"${long}"`)).toBe('value');
    expect(cutShort(`-${long}`)).toBe('value');
    // Characters, not UTF-16 units: 256 emoji are 256 characters.
    expect(cutShort('😀'.repeat(MAX_VALUE_CHARS))).toBeNull();
    expect(cutShort('😀'.repeat(MAX_VALUE_CHARS + 1))).toBe('value');
  });

  /* The engine measures the value of an operator it applies, and the whole
     piece when it applies none. */
  it('measures what the engine keeps', () => {
    const value = 'x'.repeat(MAX_VALUE_CHARS - 2);
    // A sender of 254 characters is kept whole, although the piece is longer.
    expect(cutShort(`from:${value}`)).toBeNull();
    // Not an operator the engine knows, or not a value it can take: the
    // whole piece is text, and 259 characters of it is too long.
    expect(cutShort(`re:${value}`)).toBe('value');
    expect(cutShort(`is:${value}`)).toBe('value');
    // A colon inside the quotes makes no operator either.
    expect(cutShort(`"from:${value}"`)).toBe('value');
    // The spaces around a value are not part of it.
    expect(cutShort(`"  ${'x'.repeat(MAX_VALUE_CHARS)}  "`)).toBeNull();
  });

  it('holds a mailbox to the limit after lowering it', () => {
    // `İ` lowers to two characters, so 129 of them are 258.
    expect(cutShort(`in:${'İ'.repeat(128)}`)).toBeNull();
    expect(cutShort(`in:${'İ'.repeat(129)}`)).toBe('value');
    // Only a mailbox is lowered.
    expect(cutShort(`from:${'İ'.repeat(129)}`)).toBeNull();
  });

  it('says the number of terms before the length of one', () => {
    const long = 'x'.repeat(MAX_VALUE_CHARS + 1);
    expect(cutShort(`${long} ${words(MAX_CLAUSES)}`)).toBe('terms');
  });
});
