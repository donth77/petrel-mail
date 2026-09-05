import { describe, expect, it } from 'vitest';
import { addressOf, firstUnsendable, looksLikeAddress, sendable, splitRecipients } from './recipients';

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

describe('splitRecipients with quoted names', () => {
  it('keeps a comma inside double quotes as part of the name', () => {
    // The form every other client pastes for a surname-first contact. It
    // used to become two chips, one of them an address of `"Wu`.
    expect(splitRecipients('"Wu, Dana" <dana@example.com>, sam@example.com')).toEqual([
      '"Wu, Dana" <dana@example.com>',
      'sam@example.com',
    ]);
  });

  it('still splits on the separators outside quotes', () => {
    expect(splitRecipients('"A; B" <ab@example.com>; c@example.com')).toEqual([
      '"A; B" <ab@example.com>',
      'c@example.com',
    ]);
  });

  it('does not lose the rest of the field to an unclosed quote', () => {
    expect(splitRecipients('"Dana <dana@example.com>')).toEqual(['"Dana <dana@example.com>']);
  });
});

describe('looksLikeAddress with a display name', () => {
  it('judges the address inside the angle brackets', () => {
    expect(looksLikeAddress('Dana Wu <dana@example.com>')).toBe(true);
    expect(looksLikeAddress('"Wu, Dana" <dana@example.com>')).toBe(true);
    expect(looksLikeAddress('Dana Wu <dana@example>')).toBe(false);
    expect(looksLikeAddress('Dana Wu <>')).toBe(false);
  });

  it('reads the address out of an entry', () => {
    expect(addressOf('Dana Wu <dana@example.com>')).toBe('dana@example.com');
    expect(addressOf('dana@example.com')).toBe('dana@example.com');
    expect(addressOf('  dana@example.com  ')).toBe('dana@example.com');
  });
});

describe('sendable', () => {
  it('is the rule the wire applies: an @, and nothing that makes two addresses', () => {
    expect(sendable('sam@example.com')).toBe(true);
    expect(sendable('Dana Wu <dana@example.com>')).toBe(true);
    expect(sendable('"Doe, John" <j@x.example>')).toBe(true);
    expect(sendable('bob@localhost')).toBe(true);
    expect(sendable('dan')).toBe(false);
    expect(sendable('Dana Wu')).toBe(false);
    expect(sendable('<>')).toBe(false);
    expect(sendable('a b@example.com')).toBe(false);
  });
});

describe('firstUnsendable', () => {
  it('names the first entry that would be left off the envelope, in To then Cc', () => {
    expect(firstUnsendable({ to: 'sam@example.com', cc: '' })).toBeNull();
    expect(firstUnsendable({ to: 'sam@example.com, dan', cc: '' })).toBe('dan');
    expect(firstUnsendable({ to: 'sam@example.com', cc: 'Dana Wu' })).toBe('Dana Wu');
    // A trailing comma is not an entry.
    expect(firstUnsendable({ to: 'sam@example.com,', cc: '' })).toBeNull();
  });
});
