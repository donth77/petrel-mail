import { describe, expect, it } from 'vitest';
import { leavesView } from './leaves-view';

describe('leavesView for a move', () => {
  it('takes the row out of the views that are about where mail is filed', () => {
    for (const view of ['inbox', 'archive', 'sent', 'spam', 'trash', 'folder:7']) {
      expect(leavesView('move', view), view).toBe(true);
    }
  });

  it('keeps the row in the views that are about a mark on the conversation', () => {
    // A starred conversation dragged onto a folder is still starred; the row
    // vanishing from Starred read as the star being lost.
    for (const view of ['starred', 'snoozed', 'tag:Urgent']) {
      expect(leavesView('move', view), view).toBe(false);
    }
  });
});

describe('leavesView for the rest', () => {
  it('is unchanged for the bins, archiving and the marks', () => {
    expect(leavesView('delete_forever', 'starred')).toBe(true);
    expect(leavesView('trash', 'starred')).toBe(true);
    expect(leavesView('trash', 'trash')).toBe(false);
    expect(leavesView('archive', 'inbox')).toBe(true);
    expect(leavesView('archive', 'starred')).toBe(false);
    expect(leavesView('unstar', 'starred')).toBe(true);
    expect(leavesView('unstar', 'inbox')).toBe(false);
    expect(leavesView('tag', 'inbox')).toBe(false);
  });
});
