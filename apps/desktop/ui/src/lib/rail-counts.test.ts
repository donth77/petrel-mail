import { describe, expect, it } from 'vitest';
import { buildFolderTree } from './folders';
import { mailboxCount, rowCount, treeCount } from './rail-counts';
import type { Folder } from './api';

const f = (id: number, path: string): Folder => ({ id, path, role: '' });

/** The Archive tree on the user's Namecheap account, in miniature. `Yearly`
 *  holds nothing itself; `Outdated` is not a folder at all, only a rung. */
const tree = buildFolderTree(
  [
    f(84, 'Archive/Jobs'),
    f(98, 'Archive/Yearly'),
    f(100, 'Archive/Yearly/2018'),
    f(103, 'Archive/Yearly/2023'),
    f(104, 'Archive/Yearly/2023/Job Hunt 2023'),
    f(111, 'Archive/Outdated/Interviews'),
  ],
  'Archive'.length,
);
const node = (path: string) => {
  const walk = (ns: typeof tree): (typeof tree)[number] | undefined =>
    ns.map((n) => (n.path === path ? n : walk(n.children))).find(Boolean);
  return walk(tree)!;
};

const counts = {
  archive: 1,
  'folder:84': 2,
  'folder:103': 3,
  'folder:104': 4,
  'folder:111': 5,
};

describe('the number a row wears', () => {
  it('is its own when open, since the rows under it are drawn with theirs', () => {
    expect(rowCount(counts.archive, tree, true, counts)).toBe(1);
  });

  it('takes in every row beneath it when folded, at any depth', () => {
    expect(rowCount(counts.archive, tree, false, counts)).toBe(1 + 2 + 3 + 4 + 5);
  });

  it('gives a folder that holds nothing itself the mail filed under it', () => {
    const yearly = node('Archive/Yearly');
    expect(rowCount(0, yearly.children, false, counts)).toBe(3 + 4);
    expect(rowCount(0, yearly.children, true, counts)).toBe(0);
  });

  it('gives a rung that is not a folder the same', () => {
    const outdated = node('Archive/Outdated');
    expect(outdated.folder).toBeUndefined();
    expect(rowCount(0, outdated.children, false, counts)).toBe(5);
  });

  it('adds nothing for a row whose count is switched off', () => {
    // Off rows are simply absent from the map the engine sends.
    const { 'folder:103': _off, ...rest } = counts;
    expect(treeCount(node('Archive/Yearly').children, rest)).toBe(4);
  });
});

describe('the number a mailbox row wears', () => {
  // Trash on the Namecheap account: nothing unread in Trash itself, two in a
  // folder binned under it, which the Folders setting counts.
  const bin = buildFolderTree([f(135, 'Trash/workday+092026(1)')], 'Trash'.length);
  const binCounts = { 'folder:135': 2 };

  it('says nothing when the row is set to None, folded or open', () => {
    expect(mailboxCount('off', 0, bin, false, binCounts)).toBe(0);
    expect(mailboxCount('off', 0, bin, true, binCounts)).toBe(0);
  });

  it('otherwise counts what a folded row hides', () => {
    expect(mailboxCount('unread', 0, bin, false, binCounts)).toBe(2);
    expect(mailboxCount('unread', 0, bin, true, binCounts)).toBe(0);
  });
});
