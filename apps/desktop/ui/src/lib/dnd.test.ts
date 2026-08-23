import { describe, expect, it } from 'vitest';
import { acceptsDrop, draggedIds, dropMeaning } from './dnd';

describe('dropMeaning', () => {
  it('reads the destinations that mean something', () => {
    expect(dropMeaning('archive')).toEqual({ kind: 'archive' });
    expect(dropMeaning('trash')).toEqual({ kind: 'trash' });
    expect(dropMeaning('spam')).toEqual({ kind: 'spam' });
    expect(dropMeaning('starred')).toEqual({ kind: 'star' });
    expect(dropMeaning('inbox')).toEqual({ kind: 'move', role: 'inbox' });
    expect(dropMeaning('tag:Urgent')).toEqual({ kind: 'tag', tag: 'Urgent' });
  });

  it('takes no drops where a drop would be a lie', () => {
    // Sent and Drafts say how a message came about, not where it is filed.
    for (const key of ['sent', 'drafts', 'outbox', 'snoozed', 'help', 'settings', 'tag:', '']) {
      expect(dropMeaning(key), key).toBeNull();
    }
  });
});

describe('acceptsDrop', () => {
  it('declines the view you are already looking at', () => {
    expect(acceptsDrop('archive', 'archive')).toBe(false);
    expect(acceptsDrop('tag:Urgent', 'tag:Urgent')).toBe(false);
  });

  it('accepts a real destination from anywhere else', () => {
    expect(acceptsDrop('archive', 'inbox')).toBe(true);
    expect(acceptsDrop('inbox', 'archive')).toBe(true);
    expect(acceptsDrop('tag:Urgent', 'inbox')).toBe(true);
  });

  it('still declines what takes no drops', () => {
    expect(acceptsDrop('sent', 'inbox')).toBe(false);
  });
});

describe('draggedIds', () => {
  it('takes the whole selection when the dragged row is in it', () => {
    expect(draggedIds(2, new Set([1, 2, 3])).sort()).toEqual([1, 2, 3]);
  });

  it('takes only the dragged row when it is outside the selection', () => {
    expect(draggedIds(9, new Set([1, 2, 3]))).toEqual([9]);
  });

  it('takes the row when nothing is selected', () => {
    expect(draggedIds(4, new Set())).toEqual([4]);
  });
});
