import type { Folder } from './api';

/**
 * Where a folder lands when it is dragged to Archive or Trash.
 *
 * Ordinarily that is the role folder's own path — Namecheap's `Archive` takes
 * children happily, and `Archive/2026` reads as exactly what it is. Gmail is
 * the exception: its archive wears the reserved name `[Gmail]/All Mail`, and
 * while the server will technically accept a rename into that namespace, the
 * result is a junk label other clients render raw — which is how a dragged
 * folder once surfaced as a literal `[Gmail]` tree in the rail. For those
 * accounts the anchor is an ordinary label beside the system ones: `Archive`
 * or `Trash`, which Gmail accepts, auto-creating the parent.
 *
 * Undefined when the account has no folder wearing the role at all — with no
 * anchor there is nowhere to nest, and the drop is not offered.
 */
export function nestableRolePath(
  folders: Folder[],
  role: 'archive' | 'trash',
): string | undefined {
  const fallback = role === 'archive' ? 'Archive' : 'Trash';
  const path = folders.find((f) => f.role === role)?.path;
  // No folder wears the role at all — Namecheap marks no \Archive — but a
  // plain top-level folder with the role's own name is the same place by
  // convention, and it is where this app has been filing.
  if (!path) {
    return folders.some((f) => !f.role && f.path === fallback) ? fallback : undefined;
  }
  if (path.startsWith('[Gmail]')) return fallback;
  return path;
}

/** Whether a path sits at or under an anchor, on either separator. */
export function underAnchor(path: string, anchor: string | undefined): boolean {
  return (
    anchor !== undefined &&
    (path === anchor ||
      (path.startsWith(anchor) && (path[anchor.length] === '/' || path[anchor.length] === '.')))
  );
}

/**
 * The character this account's server nests folders with.
 *
 * Read off the hierarchy the server actually reported rather than guessed
 * from the punctuation in a name. Guessing is what went wrong: splitting a
 * path on both `/` and `.` turns a folder honestly named `example.com` into
 * `com`, and a move built from that leaf renames the folder as a side effect
 * of filing it.
 *
 * So real nesting is the first answer — a folder whose path is another
 * folder's followed by a slash proves the slash. Failing that, a path under
 * `INBOX.` is the Maildir++ convention every dot-delimited server follows,
 * and is the only evidence trusted for a dot: a dot between two folders that
 * merely share a prefix is far more likely to be part of a name.
 *
 * `/` is the answer whenever nothing settles it, which is also what the rest
 * of the app has always assumed — so an unfamiliar server is no worse off
 * than it was.
 */
export function folderDelimiter(folders: ReadonlyArray<{ path: string }>): string {
  for (const child of folders) {
    for (const parent of folders) {
      if (parent.path.length >= child.path.length || !child.path.startsWith(parent.path)) continue;
      if (child.path[parent.path.length] === '/') return '/';
    }
  }
  return folders.some((f) => f.path.startsWith('INBOX.')) ? '.' : '/';
}

/** A folder's own name: the last segment of its path. */
export function folderLeaf(path: string, delim: string): string {
  const at = path.lastIndexOf(delim);
  return at === -1 ? path : path.slice(at + delim.length);
}

/** Where a folder lands when it is nested under `parent` — '' for top level. */
export function movedFolderPath(
  folders: ReadonlyArray<{ path: string }>,
  folder: { path: string },
  parent: string,
): string {
  const delim = folderDelimiter(folders);
  const leaf = folderLeaf(folder.path, delim);
  return parent ? `${parent}${delim}${leaf}` : leaf;
}

/**
 * Whether this account's folders are labels rather than places.
 *
 * On Gmail a message carries several at once and lives in All Mail besides,
 * so removing one removes a label and nothing else: the mail keeps its other
 * labels, stays in All Mail, and is still there afterwards. Every message in
 * a user label on the account this was written against also carried the All
 * Mail placement, so nothing here is tombstoned either.
 *
 * That is the opposite of a plain IMAP server, where deleting a folder
 * deletes the mail inside it — which is why deleting is routed through the
 * Trash there and why the two need different words in the same dialog.
 *
 * The `[Gmail]` namespace is the signature; the sync layer's own test is the
 * hostname, which the UI cannot see.
 */
export function foldersAreLabels(folders: Folder[]): boolean {
  return folders.some((f) => f.path.startsWith('[Gmail]'));
}

/**
 * Whether a folder moved to this account's bin would actually be in the bin.
 *
 * On Gmail it would not, and the reason is worth stating exactly, because the
 * obvious guess is wrong. `[Gmail]/Trash` does not *refuse* a child — the
 * server accepts `CREATE "[Gmail]/Trash/x"` and the folder is selectable
 * afterwards. What it hands back is an ordinary label whose name merely
 * begins `[Gmail]/Trash/`. Gmail's labels are flat and the slashes are
 * cosmetic, so the bin is not its parent in any sense that matters.
 *
 * Measured against the real account rather than assumed
 * (`live_folder_ops.rs`): a message appended into that child is found in the
 * child and *not* found in `[Gmail]/Trash`. It is not deleted, not pending
 * purge, and emptying the Trash does not touch it. The plain `Trash` label
 * the app used instead behaves the same way, and reads better in other
 * clients, which is why it was chosen — but neither is the bin.
 *
 * So the bin is not offered for folders on those accounts, and Delete is the
 * only thing on the menu, because it is the only wording that is true. This
 * fails silently rather than loudly, which is what makes it worth a rule:
 * every destination the server accepts here looks like it worked.
 *
 * Archive is deliberately left alone. A label called `Archive` holding your
 * folder is exactly what filing away means on Gmail, so nothing is claimed
 * there that is not so.
 */
export function binTakesFolders(folders: Folder[]): boolean {
  const role = folders.find((f) => f.role === 'trash')?.path;
  if (role !== undefined && role.startsWith('[')) return false;
  return nestableRolePath(folders, 'trash') !== undefined;
}

/**
 * Where a folder lands in the Trash, given what is already in there.
 *
 * A bin collects things that were never meant to share a namespace, so two
 * folders called Receipts end up in it sooner or later. IMAP has no opinion
 * about that — RENAME onto an occupied name is simply refused, with
 * `[ALREADYEXISTS] Target mailbox already exists`, and the folder stays where
 * it was while the toast showed the user a line of protocol. So the second
 * one is numbered, the way a bin has always handled it.
 *
 * Undefined when the account has no folder wearing the role at all: with no
 * anchor there is nowhere to nest, and the caller deletes outright instead.
 */
export function binDestination(folders: Folder[], folder: Folder): string | undefined {
  if (!binTakesFolders(folders)) return undefined;
  const anchor = nestableRolePath(folders, 'trash');
  if (anchor === undefined) return undefined;
  const delim = folderDelimiter(folders);
  const leaf = folderLeaf(folder.path, delim);
  // Case-insensitively, because the store matches folder names that way and
  // a server on a case-insensitive filesystem does too. The cost of being
  // wrong is a number nobody needed; the cost the other way is the refusal
  // this exists to avoid.
  const taken = new Set(
    folders.filter((f) => f.id !== folder.id).map((f) => f.path.toLowerCase()),
  );
  let name = leaf;
  for (let n = 2; taken.has(`${anchor}${delim}${name}`.toLowerCase()); n += 1) {
    name = `${leaf} ${n}`;
  }
  return `${anchor}${delim}${name}`;
}

/**
 * The folders mail can actually be filed into.
 *
 * Extracted because it was written once, inside the move picker, and the rules
 * pane then built its own destination list from the raw folder list — which on
 * a real account is a different question with a very different answer. On the
 * mailbox this was written against: 110 folders in all, 38 of them filable.
 * The other 72 are the role mailboxes, the Gmail anchor labels, and fifty-odd
 * dead alias folders sitting in the bin. A rule offering any of those is a rule
 * that quietly loses mail, so the two lists must be one list.
 *
 * Role mailboxes go because they all have verbs of their own — Archive, Trash,
 * Spam, Move to Inbox — and offering their raw server paths (`INBOX`,
 * `[Gmail]/All Mail`) puts rows in the list that exist nowhere else in the app.
 * The anchors themselves go for the same reason: mail already has Archive and
 * Trash as verbs, and a row of the same name is the same place twice. They
 * still get drawn, as the rung their children hang from.
 *
 * What is *in* the bin stays. It used to go — fifty dead alias folders made the
 * list unreadable — but that was a complaint about length, and the tree answers
 * it better: the bin arrives folded, one line, and opens if you want it.
 *
 * The move picker additionally drops the folder you are already looking at.
 * That one is about the view, not about the folder, so it stays at the call
 * site — a rule has no view to be standing in.
 */
export function filableFolders(folders: Folder[]): Folder[] {
  const archiveAnchor = nestableRolePath(folders, 'archive');
  const trashAnchor = nestableRolePath(folders, 'trash');
  return folders.filter((f) => !f.role && f.path !== archiveAnchor && f.path !== trashAnchor);
}

/** One rung of the folder hierarchy the paths already spell. */
export type FolderNode = {
  /** The one segment this rung adds. */
  label: string;
  /** The whole path down to here. */
  path: string;
  /** Absent on a rung that is not itself a folder — a name that only exists
   *  because something below it does. */
  folder?: Folder;
  children: FolderNode[];
};

/**
 * The hierarchy the paths already spell, drawn as a tree.
 *
 * Lifted out of the rail so the pickers can draw the same shape rather than
 * grow a second implementation of it. `Archive/Yearly/2023` is how people
 * actually file, and a flat list of leaf names turned forty filed years into
 * anonymous siblings — a picker has exactly that problem too.
 *
 * `consumed` is how much of each path is already accounted for by whatever the
 * tree hangs under: the rail draws the archived folders beneath the Archive
 * mailbox row, so their paths arrive with the anchor already spoken for. Zero
 * for a tree that starts at the top.
 *
 * All of `folders` must share that prefix; the caller partitions.
 */
export function buildFolderTree(folders: Folder[], consumed = 0): FolderNode[] {
  const roots: FolderNode[] = [];
  const attach = (path: string, level: FolderNode[]): FolderNode => {
    let prefix = consumed > 0 ? path.slice(0, consumed) : '';
    const segs = path.slice(consumed > 0 ? consumed + 1 : 0).split(/[/.]/);
    let node: FolderNode | undefined;
    for (const seg of segs) {
      prefix = prefix ? `${prefix}${path[prefix.length]}${seg}` : seg;
      node = level.find((n) => n.path === prefix);
      if (!node) {
        node = { label: seg, path: prefix, children: [] };
        // Appended in the order the folders arrive, which is the order the
        // engine sorted them into: anything dragged first, in the arrangement
        // chosen, then everything untouched, still alphabetical.
        //
        // This used to sort alphabetically on every insert, which is where a
        // drag went to die — the engine stored the new order faithfully and
        // the tree threw it away on the way to the screen. Sorting here is
        // still the easiest way to break dragging, wherever the tree is drawn.
        level.push(node);
      }
      level = node.children;
    }
    return node!;
  };
  for (const f of folders) attach(f.path, roots).folder = f;
  // A rung invented for a path whose folder never arrived, and which nothing
  // below it justifies either, is a row for a place that does not exist.
  const prune = (ns: FolderNode[]): FolderNode[] =>
    ns
      .map((n) => ({ ...n, children: prune(n.children) }))
      .filter((n) => n.folder || n.children.length > 0);
  return prune(roots);
}

/** A tree row, flattened back out for a list that draws depth as indentation. */
export type FolderRow = {
  /** The folder's id, or a negative one standing in for a rung that is not a
   *  folder — unique, so it can key a row, and never a thing you can choose. */
  id: number;
  /** The full path. Still what a search matches against. */
  path: string;
  depth: number;
  /** True for a rung that only exists to hold the ones under it. */
  container: boolean;
  /** Whether anything hangs under it — which is whether it gets a chevron. */
  hasChildren: boolean;
  /** Set on the Archive and Trash rungs, which are not folders like the rest:
   *  they wear their mailbox's own glyph, they sort last, and they arrive
   *  folded. */
  anchor?: 'archive' | 'trash';
};

/**
 * The filable folders as a list that still knows its shape.
 *
 * Depth-first, so the order on screen is the order of the tree, and each row
 * carries how deep it sits. Containers come through because a picker that
 * excludes the Archive anchor but offers `Archive/Old letters` would otherwise
 * show a child indented under nothing.
 */
export function filableFolderRows(folders: Folder[]): FolderRow[] {
  const archiveAnchor = nestableRolePath(folders, 'archive');
  const trashAnchor = nestableRolePath(folders, 'trash');
  const anchorOf = (path: string): FolderRow['anchor'] =>
    path === archiveAnchor ? 'archive' : path === trashAnchor ? 'trash' : undefined;

  // Archive and Trash sink to the bottom, and everything under them with them.
  // On a real mailbox 32 of 38 filable folders sit under Archive, so leaving it
  // in alphabetical place buries the five or six you actually file into beneath
  // everything you have already dealt with. Sort is stable, so nothing else
  // moves.
  const roots = [...buildFolderTree(filableFolders(folders))].sort(
    (a, b) => Number(anchorOf(a.path) !== undefined) - Number(anchorOf(b.path) !== undefined),
  );

  const rows: FolderRow[] = [];
  let placeholder = 0;
  const walk = (ns: FolderNode[], depth: number) => {
    for (const n of ns) {
      rows.push({
        id: n.folder ? n.folder.id : (placeholder -= 1),
        path: n.path,
        depth,
        container: !n.folder,
        hasChildren: n.children.length > 0,
        anchor: anchorOf(n.path),
      });
      walk(n.children, depth + 1);
    }
  };
  walk(roots, 0);
  return rows;
}

/**
 * The ids the move picker's two verb rows answer to.
 *
 * Archive and Trash are not folders you file into, they are things you do, and
 * on Gmail the difference is not cosmetic: archiving removes the Inbox label,
 * while moving a message into `[Gmail]/All Mail` over IMAP is a different
 * operation the server will not do for you. So the rows carry a sentinel
 * rather than a folder id, and the picker's caller reads them as verbs.
 *
 * Far below the placeholders `filableFolderRows` hands out for rungs, which
 * count down from -1, so the two schemes cannot meet.
 */
export const ARCHIVE_VERB = -1_000_001;
export const TRASH_VERB = -1_000_002;

/** Whether a path sits under any of these folded-away ones. */
export function underAnyClosed(path: string, closed: ReadonlySet<string>): boolean {
  for (const c of closed) if (path !== c && underAnchor(path, c)) return true;
  return false;
}

/**
 * A path split into the context you can afford to lose and the name you cannot.
 *
 * `parent` keeps its trailing delimiter, so the two halves still concatenate
 * back to what was given — the pickers rely on that to map fuzzy-match indices
 * onto the two spans they draw.
 *
 * Split on the last `/` or `.`, which is how `attach` in the rail already reads
 * a path: either character nests, and which one it is belongs to the server.
 * A name with no separator in it is all leaf, which is also the right answer
 * for a tag.
 */
export function splitPath(path: string): { parent: string; leaf: string } {
  const at = Math.max(path.lastIndexOf('/'), path.lastIndexOf('.'));
  if (at < 0) return { parent: '', leaf: path };
  return { parent: path.slice(0, at + 1), leaf: path.slice(at + 1) };
}

/** Whether the server refused because something of that name is already there. */
export function nameIsTaken(e: unknown): boolean {
  return /alreadyexists|already exists/i.test(String(e));
}

/**
 * The folder list as it will be once a move lands, so the tree can be drawn
 * before the server is asked.
 *
 * A move is a RENAME, and a RENAME takes the subtree with it: every descendant
 * is rebuilt on the new prefix, exactly as the store cascades it. Redrawing
 * only the dragged folder would leave its children hanging off a parent that
 * no longer exists, which is a worse picture than not moving at all.
 *
 * Returns the array it was given when nothing moves, so a caller can tell a
 * move that changed something from one that did not.
 */
export function movedFolders(folders: Folder[], folderId: number, to: string): Folder[] {
  const moving = folders.find((f) => f.id === folderId);
  if (!moving || moving.path === to) return folders;
  const delim = folderDelimiter(folders);
  const prefix = `${moving.path}${delim}`;
  return folders.map((f) => {
    if (f.id === folderId) return { ...f, path: to };
    if (!f.path.startsWith(prefix)) return f;
    return { ...f, path: `${to}${delim}${f.path.slice(prefix.length)}` };
  });
}
