import { describe, expect, it } from 'vitest';
import { LIST_PAGE, shouldRequestNextPage } from './list-page';

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
