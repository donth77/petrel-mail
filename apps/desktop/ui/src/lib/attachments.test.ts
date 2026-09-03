import { describe, expect, it } from 'vitest';
import { ATTACHMENT_LIMIT, encodedSize, fits, pickAttachments, stageDropped } from './attachments';

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

describe('stageDropped', () => {
  const file = (name: string, size: number) =>
    ({ name, size, arrayBuffer: async () => new ArrayBuffer(size) }) as unknown as File;
  const stage = async (name: string, bytes: Uint8Array) => ({
    path: `/staged/${name}`,
    name,
    size: bytes.byteLength,
  });

  it('stages what fits and reports what does not', async () => {
    const res = await stageDropped([file('ok.pdf', 1000), file('huge.mov', ATTACHMENT_LIMIT)], [], stage);
    expect(res.kept.map((a) => a.name)).toEqual(['ok.pdf']);
    expect(res.rejected).toEqual(['huge.mov']);
  });

  it('counts what is already attached', async () => {
    const existing = [{ path: '/a', name: 'a.bin', size: ATTACHMENT_LIMIT - 2000 }];
    const res = await stageDropped([file('b.bin', 100_000)], existing, stage);
    expect(res.kept).toHaveLength(1);
    expect(res.rejected).toEqual(['b.bin']);
  });

  it('never writes down a file it is going to refuse', async () => {
    // A 400MB video dropped by accident should be refused, not copied into the
    // application's storage first and refused second.
    const staged: string[] = [];
    await stageDropped([file('huge.mov', ATTACHMENT_LIMIT * 2)], [], async (n, b) => {
      staged.push(n);
      return { path: `/staged/${n}`, name: n, size: b.byteLength };
    });
    expect(staged).toEqual([]);
  });
});

describe('pickAttachments', () => {
  const info = async (paths: string[]) =>
    paths.map((path) => ({ path, name: path.split('/').pop() || path, size: 1024 }));

  it('answers null for a cancelled picker, which is not a failure', async () => {
    expect(await pickAttachments([], info, async () => [])).toBeNull();
  });

  it('keeps what the shell picked, once each', async () => {
    const already = { path: '/a.pdf', name: 'a.pdf', size: 1024 };
    const res = await pickAttachments([already], info, async () => ['/a.pdf', '/b.pdf']);
    expect(res?.kept.map((a) => a.path)).toEqual(['/a.pdf', '/b.pdf']);
    expect(res?.rejected).toEqual([]);
  });
});
