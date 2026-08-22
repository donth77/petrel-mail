import { describe, expect, it } from 'vitest';
import { ATTACHMENT_LIMIT, encodedSize, fits } from './attachments';

describe('encodedSize', () => {
  it('is always at least the raw size', () => {
    // Understating it would let an oversized file through the check and fail
    // at send, which is the failure this arithmetic exists to prevent.
    for (const n of [0, 1, 2, 3, 76, 1024, 3_000_000, ATTACHMENT_LIMIT]) {
      expect(encodedSize(n), `shrank at ${n}`).toBeGreaterThanOrEqual(n);
    }
  });

  it('accounts for base64 growing a file by about a third', () => {
    const raw = 3_000_000;
    const encoded = encodedSize(raw);
    expect(encoded).toBeGreaterThan(raw * 1.3);
    expect(encoded).toBeLessThan(raw * 1.4);
  });
});

describe('fits', () => {
  it('counts what is already attached, not just the new file', () => {
    // Two files each comfortably under the limit can exceed it together, and
    // checking them one at a time is how that gets missed.
    const big = { path: '/a', name: 'a', size: 15 * 1024 * 1024 };
    expect(fits([], big.size)).toBe(true);
    expect(fits([big], big.size)).toBe(false);
  });

  it('allows a file that fits with room to spare', () => {
    expect(fits([], 1024)).toBe(true);
  });

  it('refuses a single file over the limit', () => {
    expect(fits([], ATTACHMENT_LIMIT + 1)).toBe(false);
  });

  it('refuses a file that only exceeds the limit once encoded', () => {
    // 20MB on disk is about 27MB on the wire. A check against the raw size
    // would wave this through.
    expect(fits([], 20 * 1024 * 1024)).toBe(false);
  });
});
