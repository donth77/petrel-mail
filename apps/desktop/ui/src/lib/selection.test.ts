import { describe, expect, it } from 'vitest';
import { extend, prune, targets, toggle } from './selection';

const order = [1, 2, 3, 4, 5];

describe('targets', () => {
  it('is the selection when there is one', () => {
    expect(targets(new Set([2, 4]), 1).sort()).toEqual([2, 4]);
  });

  it('falls back to what is highlighted', () => {
    // Without this, every shortcut stops working the moment nothing is ticked
    // — which is most of the time.
    expect(targets(new Set(), 3)).toEqual([3]);
  });

  it('is empty when there is nothing to act on', () => {
    expect(targets(new Set(), null)).toEqual([]);
  });

  it('ignores the highlight once a selection exists', () => {
    // Acting on both would archive a conversation the user never ticked.
    expect(targets(new Set([2]), 5)).toEqual([2]);
  });
});

describe('toggle', () => {
  it('adds and removes', () => {
    expect([...toggle(new Set(), 3)]).toEqual([3]);
    expect([...toggle(new Set([3]), 3)]).toEqual([]);
  });

  it('does not mutate what it was given', () => {
    const before = new Set([1]);
    toggle(before, 2);
    expect([...before]).toEqual([1]);
  });
});

describe('extend', () => {
  it('selects the whole range from the anchor', () => {
    expect([...extend(new Set([2]), order, 2, 4)]).toEqual([2, 3, 4]);
  });

  it('works backwards', () => {
    expect([...extend(new Set([4]), order, 4, 2)]).toEqual([2, 3, 4]);
  });

  it('shrinks when the direction reverses', () => {
    // Growing to 5 then back to 3 should leave 2..3, not a trail of everything
    // the cursor ever touched.
    const grown = extend(new Set([2]), order, 2, 5);
    expect([...grown]).toEqual([2, 3, 4, 5]);
    expect([...extend(grown, order, 2, 3)]).toEqual([2, 3]);
  });

  it('starts a selection when there is no anchor yet', () => {
    expect([...extend(new Set(), order, null, 3)]).toEqual([3]);
  });

  it('ignores an id that is not in the list', () => {
    expect([...extend(new Set([1]), order, 1, 99)]).toEqual([1]);
  });
});

describe('prune', () => {
  it('drops ids whose rows have gone', () => {
    // Archiving three of five leaves a selection pointing at rows that no
    // longer exist; the next action would target nothing and look broken.
    expect([...prune(new Set([1, 3, 9]), order)]).toEqual([1, 3]);
  });

  it('leaves a valid selection alone', () => {
    expect([...prune(new Set([2, 4]), order)]).toEqual([2, 4]);
  });
});
