import type { Folder } from './api';
import { filableFolderRows } from './folders';

/** One thing the Storage pane can be asked to export. */
export type ScopeRow = {
  /** The view key the engine parses — `inbox`, `folder:12`, `tag:urgent`. */
  view: string;
  /** What the row reads as, and what a search matches against. */
  label: string;
  depth?: number;
  container?: boolean;
  hasChildren?: boolean;
  anchor?: 'archive' | 'trash';
  colour?: string;
};

/** The rows of a rung's subtree: the rung's own index, and one past its last child. */
function blockOf(
  rows: ReturnType<typeof filableFolderRows>,
  role: 'archive' | 'trash',
): { at: number; end: number } | null {
  const at = rows.findIndex((r) => r.anchor === role);
  if (at < 0) return null;
  let end = at + 1;
  // Depth-first, so everything deeper that follows is underneath it, and the
  // first row back at the rung's own depth is its next sibling.
  while (end < rows.length && rows[end].depth > rows[at].depth) end += 1;
  return { at, end };
}

/**
 * Everything one account can be asked to export, in the order it is drawn.
 *
 * The mailboxes first, then folders, then tags — with one join that is the
 * whole point of this function. Archive and Trash are both a mailbox and a
 * place other folders hang under, and drawn naively they appear twice: once in
 * the mailbox list as a choice, and again in the folder tree as the greyed
 * rung holding their children. Two rows with the same name, one of them
 * unchoosable, is the sort of thing that reads as a bug.
 *
 * So the rung is folded into the mailbox row: one Archive, which exports the
 * whole archive when chosen and opens its subfolders when its chevron is. Its
 * children follow it there rather than in the folder section, which keeps the
 * mailbox in the position someone looks for it in — and keeps that position
 * the same whether or not this account has filed anything under it.
 *
 * The merged row takes the *anchor's path* as its label rather than the
 * mailbox's word. Folding is keyed on labels — a child is hidden when a folded
 * row's label is a prefix of its own — so a translated word in that slot would
 * leave the chevron with nothing to hide. In English the two are the same
 * string; elsewhere the row reads as the folder's real name on the server,
 * which is what the rest of the tree shows too.
 */
export function exportScopes(
  mailboxes: { view: string; label: string }[],
  folders: Folder[],
  tags: { id: number; name: string; colour: string }[],
): ScopeRow[] {
  const rows = filableFolderRows(folders);
  const blocks = { archive: blockOf(rows, 'archive'), trash: blockOf(rows, 'trash') };
  const claimed = new Set<number>();
  for (const b of [blocks.archive, blocks.trash]) {
    if (b) for (let i = b.at; i < b.end; i += 1) claimed.add(i);
  }

  const folderRow = (r: (typeof rows)[number]): ScopeRow => ({
    // A rung is never chosen, so it never needs a view.
    view: r.container ? '' : `folder:${r.id}`,
    label: r.path,
    depth: r.depth,
    container: r.container || undefined,
    hasChildren: r.hasChildren || undefined,
    anchor: r.anchor,
  });

  const out: ScopeRow[] = [];
  for (const m of mailboxes) {
    const b = m.view === 'archive' ? blocks.archive : m.view === 'trash' ? blocks.trash : null;
    const head = b ? rows[b.at] : null;
    out.push({
      view: m.view,
      label: head ? head.path : m.label,
      hasChildren: head?.hasChildren || undefined,
      anchor: head?.anchor,
    });
    if (b) for (let i = b.at + 1; i < b.end; i += 1) out.push(folderRow(rows[i]));
  }
  for (let i = 0; i < rows.length; i += 1) {
    if (!claimed.has(i)) out.push(folderRow(rows[i]));
  }
  for (const tag of tags) {
    out.push({ view: `tag:${tag.name}`, label: tag.name, colour: tag.colour });
  }
  return out;
}
