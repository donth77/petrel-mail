import { describe, expect, it } from 'vitest';
import { mergeOrder } from './reorder';

describe('mergeOrder', () => {
  it('keeps the dragged order for the rows that were on screen', () => {
    expect(mergeOrder([1, 2, 3], [3, 1, 2])).toEqual([3, 1, 2]);
  });

  it('leaves rows nobody could see where they were', () => {
    // 2 is inside a folded subtree, so the drag never saw it. Dragging 3 above
    // 1 must not move 2, and must not renumber it into somebody else's slot.
    expect(mergeOrder([1, 2, 3], [3, 1])).toEqual([3, 2, 1]);
  });

  it('returns every id exactly once, which is what makes the numbering total', () => {
    const full = [10, 11, 12, 13, 14];
    const out = mergeOrder(full, [14, 10]);
    expect([...out].sort((a, b) => a - b)).toEqual(full);
  });

  it('ignores an id the list no longer holds', () => {
    // Deleted in another window between the drag starting and landing.
    expect(mergeOrder([1, 2], [2, 99, 1])).toEqual([2, 1]);
  });

  it('is a no-op when nothing was rearranged', () => {
    expect(mergeOrder([4, 5, 6], [4, 5, 6])).toEqual([4, 5, 6]);
  });

  it('handles an empty drag', () => {
    expect(mergeOrder([7, 8], [])).toEqual([7, 8]);
  });
});
