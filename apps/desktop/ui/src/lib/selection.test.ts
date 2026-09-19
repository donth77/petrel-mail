import { describe, expect, it } from 'vitest';
import { extend, facing, prune, rowsOf, tagsOnAll, targets, toggle } from './selection';

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
    expect([...extend(new Set([4]), order, 4, 2)].sort()).toEqual([2, 3, 4]);
  });

  it('shrinks when the direction reverses', () => {
    // Growing to 5 then back to 3 should leave 2..3, not a trail of everything
    // the cursor ever touched. The caller says where the last range ended.
    const grown = extend(new Set([2]), order, 2, 5);
    expect([...grown]).toEqual([2, 3, 4, 5]);
    expect([...extend(grown, order, 2, 3, 5)]).toEqual([2, 3]);
  });

  it('keeps rows picked outside the range', () => {
    // Check 1, check 4, then reach back to 2: 1 is not part of the range and
    // is not the caller's to lose.
    expect([...extend(new Set([1, 4]), order, 4, 2)].sort()).toEqual([1, 2, 3, 4]);
  });

  it('redraws only the range it drew last', () => {
    const first = extend(new Set([1]), order, 3, 5);
    expect([...first].sort()).toEqual([1, 3, 4, 5]);
    expect([...extend(first, order, 3, 4, 5)].sort()).toEqual([1, 3, 4]);
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

describe('facing', () => {
  const read = { unread: false, starred: false };
  const unread = { unread: true, starred: false };
  const starred = { unread: false, starred: true };

  it('is unread while anything in the group is, so the offer is Mark as read', () => {
    // Right-clicking a read row inside a mixed selection offered "Mark as
    // unread" and turned the unread rows unread along with the rest.
    expect(facing([read, read, unread]).unread).toBe(true);
    expect(facing([unread, read]).unread).toBe(true);
  });

  it('is read only when every conversation is', () => {
    expect(facing([read, read]).unread).toBe(false);
  });

  it('is starred only when every conversation is, so a mixed group is offered Star', () => {
    expect(facing([starred, starred]).starred).toBe(true);
    expect(facing([starred, read]).starred).toBe(false);
  });

  it('matches the row itself when there is only one', () => {
    expect(facing([unread])).toEqual({ unread: true, starred: false });
    expect(facing([starred])).toEqual({ unread: false, starred: true });
  });

  it('offers nothing to reverse for an empty group', () => {
    expect(facing([])).toEqual({ unread: false, starred: false });
  });
});

describe('rowsOf', () => {
  const items = [
    { id: 1, thread_id: 10 },
    { id: 2, thread_id: 20 },
    { id: 3, thread_id: 30 },
  ];

  it('finds rows by their own id or by conversation id, in the order asked', () => {
    expect(rowsOf([3, 10], items).map((r) => r.id)).toEqual([3, 1]);
  });

  it('skips ids the list no longer holds', () => {
    // A selection can outlive a row that moved; the action must not.
    expect(rowsOf([2, 99], items).map((r) => r.id)).toEqual([2]);
  });
});

describe('tagsOnAll', () => {
  const urgent = { name: 'Urgent' };
  const later = { name: 'Later' };

  it('is only the tags every conversation carries', () => {
    // Urgent on one of two: offering to remove it took it off that one and
    // gave the other nothing. Off for the group means the offer is to apply.
    expect(tagsOnAll([{ tags: [urgent, later] }, { tags: [later] }])).toEqual(new Set(['Later']));
  });

  it('is all of them when every conversation carries them', () => {
    expect(tagsOnAll([{ tags: [urgent] }, { tags: [urgent] }])).toEqual(new Set(['Urgent']));
  });

  it('matches the row itself when there is only one', () => {
    expect(tagsOnAll([{ tags: [urgent, later] }])).toEqual(new Set(['Urgent', 'Later']));
  });

  it('is empty for an empty group', () => {
    expect(tagsOnAll([])).toEqual(new Set());
  });
});
