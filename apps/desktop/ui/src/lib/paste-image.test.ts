import { describe, expect, it } from 'vitest';
import { pastedImages } from './paste-image';

/** Just enough of a DataTransfer for the policy to read. */
function clipboard(items: { kind: string; type: string }[], types: string[] = []) {
  return {
    types: [...types, ...items.map((i) => i.type)],
    items: items.map((i) => ({
      ...i,
      getAsFile: () => (i.kind === 'file' ? new File([''], 'x', { type: i.type }) : null),
    })),
  } as unknown as DataTransfer;
}

describe('pastedImages', () => {
  it('a screenshot on the clipboard is an image paste', () => {
    const files = pastedImages(clipboard([{ kind: 'file', type: 'image/png' }]));
    expect(files).toHaveLength(1);
  });

  it('text with a picture riding along is a text paste', () => {
    // Word and Excel put a rendered image of the copied cells on the
    // clipboard next to the real text. The words are what was copied.
    const files = pastedImages(
      clipboard([{ kind: 'file', type: 'image/png' }], ['text/plain', 'text/html']),
    );
    expect(files).toHaveLength(0);
  });

  it('svg is refused — it arrives blank at the other end', () => {
    const files = pastedImages(clipboard([{ kind: 'file', type: 'image/svg+xml' }]));
    expect(files).toHaveLength(0);
  });

  it('an empty clipboard is not a paste at all', () => {
    expect(pastedImages(null)).toHaveLength(0);
  });
});
