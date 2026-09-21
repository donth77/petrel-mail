import type { FolderNode } from './folders';

type Counts = Readonly<Record<string, number>>;

/** What one row of a tree counts on its own: its folder, or nothing for a
 *  rung that only exists because something below it does. */
function own(node: FolderNode, counts: Counts): number {
  return node.folder ? (counts[`folder:${node.folder.id}`] ?? 0) : 0;
}

/** Everything the rows of these trees count, at every depth. */
export function treeCount(nodes: readonly FolderNode[], counts: Counts): number {
  return nodes.reduce((sum, n) => sum + own(n, counts) + treeCount(n.children, counts), 0);
}

/**
 * The number a row with rows under it wears.
 *
 * Open, it counts its own folder, and the rows drawn beneath count theirs.
 * Folded, those rows are not drawn, so it counts for them as well. Thunderbird
 * does the same. Without it the unread mail in Archive/Yearly/2023 was hidden
 * behind two folds, because every row starts folded.
 *
 * The sum of the rows' own numbers rather than a fresh count, so the folded
 * number is what the open rows add up to. A row whose count is switched off
 * has no number in `counts` and adds nothing.
 */
export function rowCount(
  ownCount: number,
  children: readonly FolderNode[],
  open: boolean,
  counts: Counts,
): number {
  return open ? ownCount : ownCount + treeCount(children, counts);
}
