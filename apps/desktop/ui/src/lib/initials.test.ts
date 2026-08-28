import { describe, expect, it } from 'vitest';
import { initials } from './format';

describe('avatar initials', () => {
  it('takes one letter from each of the first two words', () => {
    expect(initials('Sam Ortiz', 'sam@example.com')).toBe('SO');
    expect(initials('ada lovelace', 'ada@example.com')).toBe('AL');
  });

  it('never draws punctuation', () => {
    /* "Pluto (YC)" splits into two words whose second begins with a bracket,
       and the avatar read "P(". A qualifier in brackets is not a name, so the
       initial it would contribute is dropped rather than replaced by reaching
       past the bracket for the Y. */
    expect(initials('Pluto (YC)', 'hi@pluto.example')).toBe('P');
    expect(initials('Jane (she/her)', 'jane@example.com')).toBe('J');
    expect(initials('Ops [alerts]', 'ops@example.com')).toBe('O');
  });

  it('still gives two letters to a single name', () => {
    expect(initials('Pluto', 'hi@pluto.example')).toBe('PL');
  });

  it('falls back to the address when there is no display name', () => {
    expect(initials('', 'sam.ortiz@example.com')).toBe('SO');
    expect(initials('   ', 'x@example.com')).toBe('XE');
  });

  it('says something rather than nothing when there is nothing to say', () => {
    expect(initials('', '')).toBe('?');
    // All punctuation: no letters anywhere, so the first-two-characters
    // fallback is what is left rather than an empty circle.
    expect(initials('(( ))', 'a@b.example')).toBe('((');
  });
});
