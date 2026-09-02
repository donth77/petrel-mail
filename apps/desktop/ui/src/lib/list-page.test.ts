import { describe, expect, it } from 'vitest';
import { LIST_PAGE, prependScrollDelta, shouldRequestNextPage } from './list-page';

describe('shouldRequestNextPage', () => {
  it('does not ask while the virtual window is still in the middle', () => {
    expect(shouldRequestNextPage(20, LIST_PAGE, 0)).toBe(false);
  });

  it('asks on the first page when the window is at the tail', () => {
    expect(shouldRequestNextPage(LIST_PAGE - 1, LIST_PAGE, 0)).toBe(true);
  });

  it('stops auto-filling after two pages unless the person has scrolled', () => {
    expect(shouldRequestNextPage(LIST_PAGE * 2 - 1, LIST_PAGE * 2, 0)).toBe(false);
    expect(shouldRequestNextPage(LIST_PAGE * 2 - 1, LIST_PAGE * 2, 40)).toBe(true);
  });
});

describe('prependScrollDelta', () => {
  const items = [{ id: 10 }, { id: 20 }, { id: 30 }];

  it('is zero at the top of the list, so new mail appears above', () => {
    expect(
      prependScrollDelta({ previousHeadId: 20, items, scrollTop: 0, rowHeight: 74 }),
    ).toBe(0);
  });

  it('shifts by the number of rows inserted above the previous head', () => {
    expect(
      prependScrollDelta({ previousHeadId: 20, items, scrollTop: 200, rowHeight: 74 }),
    ).toBe(74);
    expect(
      prependScrollDelta({ previousHeadId: 30, items, scrollTop: 200, rowHeight: 74 }),
    ).toBe(148);
  });

  it('is zero when the previous head is gone — a replace, not a prepend', () => {
    expect(
      prependScrollDelta({ previousHeadId: 99, items, scrollTop: 200, rowHeight: 74 }),
    ).toBe(0);
  });
});
