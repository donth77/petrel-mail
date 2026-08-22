import { describe, expect, it } from 'vitest';
import { addresses } from './Compose';

/**
 * Recipient parsing. Forgiving on input by design: people paste addresses from
 * everywhere, and rejecting a trailing comma teaches them to distrust the field
 * rather than to type differently.
 */
describe('addresses', () => {
  it('splits on the separators people actually type', () => {
    expect(addresses('a@example.com, b@example.com')).toEqual([
      'a@example.com',
      'b@example.com',
    ]);
    expect(addresses('a@example.com; b@example.com')).toEqual([
      'a@example.com',
      'b@example.com',
    ]);
  });

  it('ignores the empty entries a trailing separator leaves behind', () => {
    expect(addresses('a@example.com,')).toEqual(['a@example.com']);
    expect(addresses('a@example.com, , b@example.com')).toEqual([
      'a@example.com',
      'b@example.com',
    ]);
  });

  it('trims, because pasted addresses arrive with whitespace', () => {
    expect(addresses('  a@example.com  ')).toEqual(['a@example.com']);
  });

  it('treats blank and whitespace-only fields as no recipients', () => {
    // This is what the send guard checks, so it has to be exactly right:
    // an "empty" field that parses to [''] would send to nobody and report
    // success.
    expect(addresses('')).toEqual([]);
    expect(addresses('   ')).toEqual([]);
    expect(addresses(' , ; ')).toEqual([]);
  });
});
