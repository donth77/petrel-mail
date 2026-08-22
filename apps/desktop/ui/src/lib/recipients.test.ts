import { describe, expect, it } from 'vitest';
import { looksLikeAddress, splitRecipients } from './recipients';

describe('splitRecipients', () => {
  it('takes the separators people actually type', () => {
    expect(splitRecipients('a@x.com, b@y.com; c@z.com')).toEqual([
      'a@x.com', 'b@y.com', 'c@z.com',
    ]);
  });

  it('drops the empties a trailing separator leaves behind', () => {
    expect(splitRecipients('a@x.com, ,')).toEqual(['a@x.com']);
    expect(splitRecipients('   ')).toEqual([]);
  });
});

describe('looksLikeAddress', () => {
  it('accepts the strange but legal', () => {
    for (const ok of [
      'a@b.co',
      'first.last+tag@sub.example.museum',
      "o'brien@example.ie",
      'me@example.co.uk',
    ]) {
      expect(looksLikeAddress(ok), ok).toBe(true);
    }
  });

  /* The point of the suspect chip is the mistake you cannot see in a
     comma-separated line — so these are the ones that must be caught. */
  it('catches what people actually get wrong', () => {
    for (const bad of [
      'Nadia Okafor',   // a name pasted instead of an address
      'nadia@',         // stopped typing
      '@example.com',   // lost the local part
      'nadia@example',  // no dot in the domain
      'a@b@example.com',
      'nadia @example.com',
      '',
    ]) {
      expect(looksLikeAddress(bad), bad).toBe(false);
    }
  });
});
