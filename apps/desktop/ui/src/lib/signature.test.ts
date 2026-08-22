import { describe, expect, it } from 'vitest';
import { SEPARATOR, startingBody } from './signature';
import type { Identity } from './api';

const identity = (over: Partial<Identity> = {}): Identity => ({
  address: 'you@example.com',
  display_name: 'You',
  signature: 'You\nNorthbay',
  signature_on_reply: false,
  ...over,
});

describe('the signature separator', () => {
  it('is exactly two hyphens and a space', () => {
    // Every client folds a signature away on this token. Three hyphens, or no
    // trailing space, and it shows as ordinary text at the bottom of every
    // message forever — a mistake nobody notices in their own outbox.
    expect(SEPARATOR).toBe('-- ');
  });

  it('appears on its own line above the signature', () => {
    const body = startingBody(identity(), false);
    const lines = body.split('\n');
    const at = lines.indexOf(SEPARATOR);
    expect(at).toBeGreaterThan(-1);
    expect(lines[at + 1]).toBe('You');
  });
});

describe('when a signature is used', () => {
  it('goes on new messages', () => {
    expect(startingBody(identity(), false)).toContain('Northbay');
  });

  it('stays off replies unless asked for', () => {
    expect(startingBody(identity(), true)).toBe('');
    expect(startingBody(identity({ signature_on_reply: true }), true)).toContain('Northbay');
  });

  it('adds nothing when there is no signature', () => {
    expect(startingBody(identity({ signature: '' }), false)).toBe('');
    expect(startingBody(identity({ signature: '   \n ' }), false)).toBe('');
    expect(startingBody(null, false)).toBe('');
  });

  it('leaves room to write above it', () => {
    const body = startingBody(identity(), false);
    expect(body.startsWith('\n\n')).toBe(true);
  });
});
