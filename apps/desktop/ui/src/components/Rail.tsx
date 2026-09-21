import type React from 'react';
import { useEffect, useRef, useState } from 'react';
import {
  ChevronDown, ChevronRight, FolderClosed,
  CircleHelp, PanelLeftClose, PanelLeftOpen, PenSquare, Plus, Search, Settings, FolderPlus, TagPlus } from 'lucide-react';
import { Fragment } from 'react';
import type { Account, Folder, SavedSearch } from '../lib/api';
import type { SectionKey } from '../lib/rail-sections';
import { Icon } from './Icon';
import { SearchMenu } from './SearchMenu';
import { t } from '../lib/strings';
import { TagMenu } from './TagMenu';
import { FolderMenu } from './FolderMenu';
import { NameDialog } from './NameDialog';
import { acceptsDrop } from '../lib/dnd';
import type { InsertPoint } from '../lib/useDrag';
import { buildFolderTree, nestableRolePaths, type FolderNode, nestableRolePath, underAnchor } from '../lib/folders';
import { MAILBOX_KEYS, MAILBOX_LOOK } from '../lib/mailboxes';
import { AccountMenu } from './AccountMenu';
import { RailFlyout } from './RailFlyout';
import { Tip } from './Tip';

/** The rail's mailbox rows, from the one map the settings pane also draws. */
const MAILBOXES = MAILBOX_KEYS.map((key) => ({
  key,
  id: MAILBOX_LOOK[key].label,
  glyph: MAILBOX_LOOK[key].glyph,
}));

type Tag = { id: number; name: string; colour: string; thread_count: number };

/** What a row with children does before anybody touches it. See `isOpen`. */
const FOLDED_AT_LAUNCH = true;

/**
 * Marks a rail destination so a drag can find it.
 *
 * A data attribute rather than event handlers: the drag hit-tests the document
 * for whatever is under the pointer, so a destination only has to be findable
 * and say which key it is. That also means a destination cannot miss a drag
 * because one of its own children swallowed the event.
 */
function dropTarget(railKey: string, view: string, over: string | null) {
  if (!acceptsDrop(railKey, view)) return {};
  return {
    'data-drop-key': railKey,
    'data-drop-over': over === railKey || undefined,
  };
}


type Props = {
  account: string;
  accounts: Account[];
  collapsed: boolean;
  onToggleCollapsed: () => void;
  onCompose: () => void;
  /** Absolute x during a drag, or a signed delta from the keyboard. */
  onResize: (xOrDelta: number) => void;
  onSwitchAccount: (index: number) => void;
  onSettings: () => void;
  onAddAccount: () => void;
  /** Conversations dropped on a destination. The rail decides where; what that
      means to the store is the caller's business. */
  /** The destination under the pointer mid-drag, so it can light up. */
  dropOver: string | null;
  /** Where a reorder would land, so the row can draw the line. */
  insertAt: InsertPoint | null;
  /** Whether anything at all is being carried. `dragActive` is narrower — it
   *  means conversations, which is what a drop target lights up for, and the
   *  local `dragging` below adds folders to that. */
  anyDrag: boolean;
  /** Outbox messages waiting on a decision. Any at all turns the row amber:
      a message that needs a person must not go unnoticed, and this is where
      you find out — the sidebar, not a dialog. */
  outboxNeedsAttention: number;
  /** Which mailboxes to draw, in order. From the sidebar arrangement, so a row
   *  somebody hid is simply absent rather than drawn and ignored. */
  mailboxOrder: string[];
  /** Which groups to draw, in order (`rail-sections.ts`). */
  sectionOrder: SectionKey[];
  /** Whether a drag is in flight, so destinations can say they will take it
      before the pointer reaches them rather than only once it arrives. */
  dragActive: boolean;
  accountColor: string;
  /** Null until the first list arrives — unknown, rather than none. */
  unread: number | null;
  /** Per-mailbox numbers, keyed by rail key. Absent means nothing to show —
   *  the engine omits empty ones rather than sending zeroes. */
  counts: Record<string, number>;
  view: string;
  /** The row to mark as the one you are looking at, which is not always the
   *  view: a search is not the mailbox it was started from, so while one is
   *  running nothing is current — unless the search *is* a saved one, and then
   *  its own row is. `view` still decides what a drop means. */
  current: string;
  tags: Tag[];
  /** Questions pinned under a name. Empty until the first one is saved. */
  searches: SavedSearch[];
  /** Every folder; the rail lists the ones the user made (no role). */
  folders: Folder[];
  onView: (v: string) => void;
  onCreateFolder: (name: string) => Promise<void>;
  /** Begins carrying a folder toward a new parent. */
  onDragFolder: (e: React.PointerEvent, folderId: number, label: string) => void;
  /** Path of the folder mid-drag, so valid destinations can say so — and so
   *  the folder itself and its descendants can decline to light up. */
  folderDragPath: string | null;
  onDeleteFolder: (folder: Folder) => void;
  /** Opens the move-destination picker for this folder. */
  onMoveFolder: (folder: Folder) => void;
  /** Asks to empty the bin. Absent in windows that do not own that. */
  onEmptyTrash?: () => void;
  /** Everything in a folder read or unread, and everything in it to the Trash.
   *  Both act on the mail, not the folder, so both take the folder itself. */
  onMarkFolderRead: (folder: Folder, read: boolean) => void;
  onTrashFolderContents: (folder: Folder) => void;
  /** Make a tag that is attached to nothing yet. Returns once it exists, so the
   *  rail can put the input away only after the work succeeded. */
  onCreateTag: (name: string) => Promise<void>;
  onColourTag: (tagId: number, colour: string) => void;
  onDeleteTag: (tag: { id: number; name: string }) => void;
  /** Begins carrying this tag towards a conversation. */
  onDragTag: (e: React.PointerEvent, tagId: number, name: string) => void;
  onDragSearch: (e: React.PointerEvent, searchId: number, name: string) => void;
  /** Asks for the rename dialog rather than editing in place: one dialog for
   *  every name the rail holds. */
  onAskRename: (what: { kind: 'search' | 'tag' | 'folder'; id: number; name: string }) => void;
  onDeleteSearch: (search: { id: number; name: string }) => void;
  /** A row a step up or down its own list. The rail knows which row was asked;
   *  how the order is saved differs per kind and is App's business. */
  onReorderRow: (kind: 'search' | 'tag' | 'folder', id: number, up: boolean) => void;
  railRef?: React.Ref<HTMLElement>;
};

export function Rail({
  account,
  accounts,
  accountColor,
  unread,
  counts,
  view,
  current,
  tags,
  searches,
  folders,
  collapsed,
  onView,
  onCreateFolder,
  onDragFolder,
  folderDragPath,
  onDeleteFolder,
  onMoveFolder,
  onEmptyTrash,
  onMarkFolderRead,
  onTrashFolderContents,
  onCreateTag,
  onColourTag,
  onDeleteTag,
  onDragTag,
  onDragSearch,
  onAskRename,
  onDeleteSearch,
  onReorderRow,
  onToggleCollapsed,
  onCompose,
  onResize,
  onSwitchAccount,
  onSettings,
  onAddAccount,
  dropOver,
  insertAt,
  anyDrag,
  dragActive,
  outboxNeedsAttention,
  mailboxOrder,
  sectionOrder,
  railRef,
}: Props) {

  // Pointer drag, with the listeners on the window rather than the handle: a
  // fast drag outruns a 6px target, and losing the pointer mid-resize leaves
  // the rail stuck at whatever width the last event happened to land on.
  // Naming a new tag. An inline field rather than a dialog: it is one short
  // string, and a modal for one word is more ceremony than the act deserves.
  const [naming, setNaming] = useState(false);
  const [namingFolder, setNamingFolder] = useState(false);
  /** What the naming field starts holding — "Parent/" for a subfolder. */
  const [folderPrefill, setFolderPrefill] = useState('');
  /** Rows folded shut by hand (true) or opened by hand (false). A path that is
   *  absent takes the default, which is folded — see FOLDED_AT_LAUNCH. */
  const [folded, setFolded] = useState<Record<string, boolean>>({});
  // Which naming dialog is up — the collapsed rail's way of asking for a
  // name without forcing itself open.
  const [namingDialog, setNamingDialog] = useState<'folder' | 'tag' | null>(null);
  // The tag being renamed, edited in place on its own row rather than in a
  // dialog: it is one short string, and the row is where you are looking.
  /** Where the archive tree roots, for the mailbox row's folder-drop. */
  const archiveRolePath = nestableRolePath(folders, 'archive');
  /** The folder a mailbox row stands for, where one exists. A row's verbs act
   *  on mail, and mail lives in a folder rather than in a view. */
  const roleFolder = (key: string) => folders.find((f) => f.role === key);
  const nameInput = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (naming) nameInput.current?.focus();
  }, [naming]);

  const startDrag = (e: React.PointerEvent) => {
    e.preventDefault();
    const move = (ev: PointerEvent) => onResize(ev.clientX);
    const up = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      document.body.classList.remove('resizing');
    };
    document.body.classList.add('resizing');
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  };

  /* The hierarchy the paths already spell, drawn as one. A mailbox tree
     like Archive/Yearly/2023 is how people actually file, and a flat list
     of leaf names turned forty filed years into anonymous siblings.
     Containers that are not themselves folders still get a row, for the
     chevron. Archived folders are kept apart: they render under the
     Archive mailbox row, not as a second Archive in the Folders section. */
  type FNode = FolderNode;
  const archivePath = archiveRolePath;
  const trashPath = nestableRolePath(folders, 'trash');
  // Three trees, because two of them hang under a mailbox row rather than at
  // the top level. The rows wearing the anchors' own names are those mailbox
  // rows' business — a second row saying Archive or Trash is the duplicate
  // this partition avoids. Order within each bucket is the order the engine
  // gave, which is the order a drag rearranged; buildFolderTree keeps it.
  const own = folders.filter((x) => !x.role);
  const under = (f: (typeof own)[number], anchor: string | undefined) =>
    underAnchor(f.path, anchor) && f.path !== anchor;
  // Every folder wearing the trash role, not only the first: an account with
  // both `Deleted Messages` and `Trash` marked as trash had everything binned
  // under `Trash/` drawn in Folders, under a "Trash" the tree made up.
  const trashPaths = nestableRolePaths(folders, 'trash');
  const inTrash = (path: string) => trashPaths.some((a) => underAnchor(path, a));
  const tree = buildFolderTree(
    own.filter((f) => !underAnchor(f.path, archivePath) && !inTrash(f.path)),
  );
  const archiveTree = buildFolderTree(
    own.filter((f) => under(f, archivePath)),
    archivePath?.length ?? 0,
  );
  // One subtree per trash anchor, each with its own prefix taken off, joined
  // under the one Trash row.
  const trashTree = trashPaths.flatMap((a) =>
    buildFolderTree(
      own.filter((f) => under(f, a)),
      a.length,
    ),
  );
  /* Every row with children starts folded, on every launch.

     The rail opens as a list of the things you filed under, not as the whole
     filing cabinet: this account has forty folders under Archive alone, and
     unrolling them pushes the folders somebody actually works in off the
     bottom of the screen. Opening one is a click; scrolling past forty is not.

     Archive and Trash were the only two that defaulted this way, which made
     the rule read as a rule about those two mailboxes rather than about
     depth. It also depended on a role: an account whose server marks no
     \Archive had no anchor to match, so the whole archive tree unrolled at
     launch anyway.

     Folds are not remembered between launches, deliberately. "Where did I
     leave the sidebar three days ago" is not a question worth restoring, and
     an app that opens the same way every time is one you can learn. */
  const isOpen = (path: string) => !(folded[path] ?? FOLDED_AT_LAUNCH);
  const archiveOpen = archivePath !== undefined && isOpen(archivePath);
  const trashOpen = trashPath !== undefined && isOpen(trashPath);

  const toggle = (path: string) =>
    setFolded((prev) => ({ ...prev, [path]: !(prev[path] ?? FOLDED_AT_LAUNCH) }));

  const dragging = dragActive || folderDragPath !== null;

  /* Which flyout a drag was picked up in, so the one you are working inside
     can stay open while the rest shut.

     A card opens beside the rail, over the message list, so it never covers
     the rail rows a drag is aiming at — the reason to shut the others is that
     a card blooming under a travelling pointer is one more surface for the
     drag to land on by accident, not that it hides anything.

     Keeping the owning card is what makes the collapsed rail reorganisable at
     all. Its rows already carry data-folder-drop and data-reorder, so a
     subtree in a card can be nested and reordered within itself — drag 2023
     onto Yearly, or into the gap above it — and those are most of what folder
     rearranging is. Shutting the card the instant the drag began took both
     away and pulled the siblings out from under the pointer.

     Only a folder drag gets the exemption. That keeps a stale origin — set by
     a press that turned out to be a click — from ever mattering: a folder drag
     is always preceded by a press on the row it carries, which sets this. */
  const [dragOrigin, setDragOrigin] = useState<string | null>(null);
  const cardSuppressed = (card: string) =>
    dragging && !(folderDragPath !== null && dragOrigin === card);

  /** `owner` is the flyout a row is being drawn inside, absent in the rail. */
  const renderNode = (
    n: FNode,
    depth: number,
    owner?: string,
    // Where this folder sits among its own siblings. A folder's order is only
    // ever relative to those, so first/last cannot be read off the flat list.
    at = 0,
    of = 1,
  ): React.ReactNode => {
    // Inside a flyout a row is an ordinary expanded row: the card is portalled
    // out of the rail, so none of the [data-collapsed] rules reach it, and it
    // has the width for a label and an indent. `dense` is therefore "drawn as
    // an icon", which is not the same question as "is the rail collapsed".
    const dense = collapsed && owner === undefined;
    // Collapsed, the rail draws roots and nothing else — the descendants are
    // the flyout's job. Expanded, the fold state decides. Inside the card
    // everything is open; see RailFlyout.
    const open = owner !== undefined || (!collapsed && isOpen(n.path));
    // The chevron hangs in the row's left padding, so the icon holds the
    // same column whether a row can fold or not — a chevron that pushed the
    // icon right made every folding root read as its neighbour's child.
    const chevron = n.children.length > 0 && !collapsed && (
      <button
        type="button"
        className="tree-toggle hanging"
        style={{ insetInlineStart: (10 + depth * 14) - 15 }}
        aria-label={open ? t('folder-fold') : t('folder-unfold')}
        aria-expanded={open}
        onClick={(e) => {
          e.stopPropagation();
          toggle(n.path);
        }}
        onPointerDown={(e) => e.stopPropagation()}
      >
        <Icon icon={open ? ChevronDown : ChevronRight} size={12} />
      </button>
    );
    // Depth is meaningless in a collapsed rail: an indented icon leaves the
    // one column everything else lines up in, so the padding only applies
    // when there is text to indent.
    const indent = dense ? undefined : ({ paddingLeft: 10 + depth * 14 } as const);
    const f = n.folder;
    const inner = f ? (
      <button
          type="button"
          className="rail-item"
          style={indent}
          aria-current={current === `folder:${f.id}` ? 'page' : undefined}
          onClick={() => onView(`folder:${f.id}`)}
          onPointerDown={(e) => {
            setDragOrigin(owner ?? null);
            onDragFolder(e, f.id, n.label);
          }}
          {...dropTarget(`folder:${f.id}`, view, dropOver)}
          data-folder-drop={f.path}
          data-reorder={`folder:${f.id}`}
          // Which edge to draw the line against. CSS puts it there; keeping
          // the decision in one attribute means the line cannot appear on
          // two rows at once.
          data-insert={insertAt?.key === `folder:${f.id}` ? insertAt.edge : undefined}
          // One merged answer, written after the spread: dropTarget only
          // knows mail drags, and its undefined used to land last and wipe
          // the folder-drag highlight off every folder row.
          data-drop-over={
            dropOver === `fdrop:${f.path}` || dropOver === `folder:${f.id}` || undefined
          }
          data-drop-ok={
            (dragActive && acceptsDrop(`folder:${f.id}`, view)) ||
            (folderDragPath !== null &&
              f.path !== folderDragPath &&
              !f.path.startsWith(`${folderDragPath}/`))
              ? true
              : undefined
          }
        >
          {chevron}
          <Icon icon={FolderClosed} />
          <span className="rail-text">{n.label}</span>
          {/* Before the count, so the two never share a corner. */}
          {!collapsed && (
            <FolderMenu
              path={f.path}
              first={at === 0}
              last={at === of - 1}
              onReorder={(up) => onReorderRow('folder', f.id, up)}
              onRename={() => onAskRename({ kind: 'folder', id: f.id, name: f.path })}
              onNewChild={() => {
                setFolderPrefill(`${f.path}/`);
                setNamingFolder(true);
              }}
              onMove={() => onMoveFolder(f)}
              onDelete={() => onDeleteFolder(f)}
              onMarkAll={(read) => onMarkFolderRead(f, read)}
              onTrashAll={() => onTrashFolderContents(f)}
            />
          )}
          {!dense && counts[`folder:${f.id}`] > 0 && (
            <span className="count">{counts[`folder:${f.id}`]}</span>
          )}
        </button>
    ) : (
      <button
        type="button"
        className="rail-item tree-container"
        style={indent}
        onClick={() => toggle(n.path)}
      >
        {chevron}
        <Icon icon={FolderClosed} />
        <span className="rail-text">{n.label}</span>
      </button>
    );
    // A collapsed row with children hands them to a flyout instead of a
    // tooltip: the path a tooltip would print is the thing the card draws
    // properly, and two hover surfaces on one icon would race each other.
    const card = `folder:${n.path}`;
    const row =
      dense && n.children.length > 0 ? (
        <RailFlyout
          key={n.path}
          label={f?.path ?? n.path}
          suppressed={cardSuppressed(card)}
          anchor={inner}
        >
          {n.children.map((c, i) => renderNode(c, 0, card, i, n.children.length))}
        </RailFlyout>
      ) : f ? (
        <Tip key={f.id} label={f.path} placement="right" when={dense}>
          {inner}
        </Tip>
      ) : (
        inner
      );
    return (
      <div key={n.path}>
        {row}
        {open && n.children.map((c, i) => renderNode(c, depth + 1, owner, i, n.children.length))}
      </div>
    );
  };

  const mailboxesSection = (
    <>
      {/* Headings, not decoration. These were plain divs, so a screen reader
          met four unexplained buttons where a sighted reader sees a labelled
          group — and heading navigation, which is how people move around a
          sidebar, had nothing to move between. */}
      <div className="rail-label" role="heading" aria-level={2}>
        {t('rail-mailboxes')}
      </div>
      {mailboxOrder
        .map((key) => MAILBOXES.find((m) => m.key === key))
        .filter((m): m is (typeof MAILBOXES)[number] => m !== undefined)
        .map((m) => {
        const subtree = m.key === 'archive' ? archiveTree : m.key === 'trash' ? trashTree : [];
        const anchor = (
          <button
            type="button"
            className="rail-item"
            aria-current={current === m.key ? 'page' : undefined}
            data-attention={m.key === 'outbox' && outboxNeedsAttention > 0 ? true : undefined}
            onClick={() => onView(m.key)}
            {...dropTarget(m.key, view, dropOver)}
            // A carried folder lands on these two as well: Archive re-nests
            // it under the archive tree, Trash deletes it — behind the same
            // confirm the menu uses, because the server deletes its mail.
            data-folder-drop={
              folderDragPath !== null && m.key === 'archive' && archiveRolePath !== undefined
                ? archiveRolePath
                : folderDragPath !== null && m.key === 'trash'
                  ? '::trash'
                  : undefined
            }
            // One merged answer, written after the spread: dropTarget's own
            // value would otherwise be overwritten with undefined during a
            // mail drag, and the row a conversation hovers over never lit.
            data-drop-over={
              dropOver === m.key ||
              (folderDragPath !== null &&
                ((m.key === 'archive' && dropOver === `fdrop:${archiveRolePath}`) ||
                  (m.key === 'trash' && dropOver === 'fdrop:::trash'))) ||
              undefined
            }
            data-drop-ok={
              (dragActive && acceptsDrop(m.key, view)) ||
              (folderDragPath !== null &&
                ((m.key === 'archive' && archiveRolePath !== undefined) || m.key === 'trash'))
                ? true
                : undefined
            }
          >
            {((m.key === 'archive' && archiveTree.length > 0) ||
              (m.key === 'trash' && trashTree.length > 0)) &&
              !collapsed &&
              (() => {
                const open = m.key === 'archive' ? archiveOpen : trashOpen;
                const anchor = m.key === 'archive' ? archivePath! : trashPath!;
                return (
                  <button
                    type="button"
                    className="tree-toggle hanging"
                    style={{ insetInlineStart: -6 }}
                    aria-label={open ? t('folder-fold') : t('folder-unfold')}
                    aria-expanded={open}
                    onClick={(e) => {
                      e.stopPropagation();
                      toggle(anchor);
                    }}
                    onPointerDown={(e) => e.stopPropagation()}
                  >
                    <Icon icon={open ? ChevronDown : ChevronRight} size={12} />
                  </button>
                );
              })()}
            <Icon icon={m.glyph} />
            <span className="rail-text">{t(m.id)}</span>
            {/* Collapsed, there is no room for a number beside a 16px icon,
                and a dot that only says "something" is not worth the pixels —
                the tooltip carries the label, and expanding carries the count. */}
            {/* The Inbox and the Archive get the same verbs a folder does,
                because they hold mail the same way. The row's own folder is
                what they act on: a mailbox key names a view, and marking a
                view read is not a thing the server understands.

                Archived folders also hang off the Archive row, so it carries
                their chevron and the way to make the first one. The ⋮ comes
                before the count, so the two never share a corner. */}
            {/* Every row with a real folder behind it gets the verbs that act
                on its mail. Starred, Snoozed and the Outbox are views rather
                than folders — there is nothing on the server to mark — so
                they carry no menu at all rather than one that cannot work.

                Sent and Spam get all three: clearing spam into the bin and
                tidying Sent are both things people do. Drafts is left out of
                the binning on purpose — a draft in the Trash is a strange
                object, and discarding drafts deserves its own verb rather
                than arriving as a side effect of this one. */}
            {['inbox', 'archive', 'sent', 'spam', 'drafts'].includes(m.key) &&
              !collapsed &&
              (() => {
                const own = roleFolder(m.key);
                if (!own) return null;
                return (
                  <FolderMenu
                    path={own.path}
                    // A mailbox wearing a tree, not a folder in a list: there is
                    // nothing to reorder it among.
                    first
                    last
                    onReorder={() => {}}
                    onNewChild={
                      m.key === 'archive' && archivePath
                        ? () => {
                            setFolderPrefill(`${archivePath}/`);
                            setNamingFolder(true);
                          }
                        : undefined
                    }
                    onMarkAll={(read) => onMarkFolderRead(own, read)}
                    onTrashAll={
                      m.key === 'drafts' ? undefined : () => onTrashFolderContents(own)
                    }
                  />
                );
              })()}
            {/* The bin's verb lives where every other folder verb lives,
                rather than in the list header: it is a thing done to a
                folder, and the header has no other actions for it to sit
                beside. Always offered, not only when the bin holds folders —
                a menu that appears and disappears is one nobody learns. */}
            {m.key === 'trash' && !collapsed && onEmptyTrash && (
              // The bin is a mailbox, not a folder among folders: nothing to
              // reorder it against.
              <FolderMenu
                path={trashPath ?? 'Trash'}
                first
                last
                onReorder={() => {}}
                onEmpty={onEmptyTrash}
              />
            )}
            {!collapsed && counts[m.key] > 0 && (
              <span className="count">{counts[m.key]}</span>
            )}
          </button>
        );
        // Archive and Trash wear their trees. Collapsed, that tree is in a
        // flyout and the tooltip would be a second hover surface on the same
        // icon saying less, so the card replaces it rather than joining it.
        const row =
          collapsed && subtree.length > 0 ? (
            <RailFlyout
              key={m.key}
              label={t(m.id)}
              suppressed={cardSuppressed(`mailbox:${m.key}`)}
              anchor={anchor}
            >
              {subtree.map((c, i) => renderNode(c, 0, `mailbox:${m.key}`, i, subtree.length))}
            </RailFlyout>
          ) : (
            <Tip key={m.key} label={t(m.id)} placement="right" when={collapsed}>
              {anchor}
            </Tip>
          );
        if (m.key === 'archive' && archiveTree.length > 0) {
          return (
            <div key={m.key}>
              {row}
              {!collapsed && archiveOpen && archiveTree.map((c, i) => renderNode(c, 1, undefined, i, archiveTree.length))}
            </div>
          );
        }
        if (m.key === 'trash' && trashTree.length > 0) {
          return (
            <div key={m.key}>
              {row}
              {!collapsed && trashOpen && trashTree.map((c, i) => renderNode(c, 1, undefined, i, trashTree.length))}
            </div>
          );
        }
        return row;
        })}
    </>
  );

  const foldersSection = (
    <>
      {/* Folders the user made, between the fixed mailboxes and the tags —
          places before labels. The header shows even with none yet, because
          the + is how the first one gets made. */}
      <div
        className="rail-label rail-label-row"
        data-folder-drop=""
        data-drop-over={dropOver === 'fdrop:' || undefined}
        data-drop-ok={folderDragPath !== null || undefined}
      >
        <span role="heading" aria-level={2}>
          {t('rail-folders')}
        </span>
        <Tip label={t('folder-new')} placement="right">
          <button
            type="button"
            className="rail-add"
            aria-label={t('folder-new')}
            // Collapsed there is no row to type into, so the + asks in a
            // dialog and the rail stays as it was. The icon says which +
            // this is, since the header text it sits beside has faded out.
            onClick={() => (collapsed ? setNamingDialog('folder') : setNamingFolder(true))}
          >
            <Icon icon={collapsed ? FolderPlus : Plus} size={13} />
          </button>
        </Tip>
      </div>
      {!collapsed && namingFolder && (
        <input
          key={folderPrefill}
          className="rail-new-tag"
          placeholder={t('folder-new-placeholder')}
          aria-label={t('folder-new')}
          autoComplete="off"
          autoFocus
          defaultValue={folderPrefill}
          onBlur={(e) => {
            const name = e.currentTarget.value.trim();
            setNamingFolder(false);
            setFolderPrefill('');
            if (name) void onCreateFolder(name);
          }}
          onKeyDown={(e) => {
            e.stopPropagation();
            if (e.key === 'Escape') {
              setNamingFolder(false);
              return;
            }
            if (e.key !== 'Enter') return;
            if (!e.currentTarget.value.trim()) {
              setNamingFolder(false);
              return;
            }
            e.currentTarget.blur();
          }}
        />
      )}
      {tree.map((n, i) => renderNode(n, 0, undefined, i, tree.length))}
      {/* The header shows even with no tags yet, because the + is how the first
          one gets made — a section that only appears once you already have one
          is a feature you cannot find.

          It stays in the layout when the rail collapses, hidden the same way
          the Mailboxes heading is. Removing it took its 37px with it and every
          tag below jumped up, while the mailboxes — whose heading only fades —
          held still. Two headings, two behaviours, one of them visibly wrong. */}
    </>
  );

  const tagsSection = (
    <>
      <div className="rail-label rail-label-row">
            <span role="heading" aria-level={2}>
              {t('rail-tags')}
            </span>
            <Tip label={t('tag-new')} placement="right">
              <button
                type="button"
                className="rail-add"
                aria-label={t('tag-new')}
                onClick={() => (collapsed ? setNamingDialog('tag') : setNaming(true))}
              >
                <Icon icon={collapsed ? TagPlus : Plus} size={13} />
              </button>
            </Tip>
          </div>
      {/* The field itself only exists while the rail is open: there is nowhere
          to type in a collapsed one. */}
      {!collapsed && naming && (
            <input
              ref={nameInput}
              className="rail-new-tag"
              placeholder={t('tag-new-placeholder')}
              aria-label={t('tag-new')}
              autoComplete="off"
              // Committed on the way out, not discarded. Typing a name and
              // clicking elsewhere used to lose it silently, which reads as the
              // tag having been created and then vanished.
              onBlur={(e) => {
                const name = e.currentTarget.value.trim();
                setNaming(false);
                if (name) void onCreateTag(name);
              }}
              onKeyDown={(e) => {
                // Stopped here so the app's single-key shortcuts do not fire
                // while a tag is being named — typing "e" should not archive.
                e.stopPropagation();
                if (e.key === 'Escape') {
                  setNaming(false);
                  return;
                }
                if (e.key !== 'Enter') return;
                const name = e.currentTarget.value.trim();
                if (!name) {
                  setNaming(false);
                  return;
                }
                // Blur does the creating; this only ends the editing, so a
                // name is not created once by Enter and again by the blur that
                // Enter causes.
                e.currentTarget.blur();
              }}
            />
          )}

      {tags.map((tag, at) => (
            <Tip key={tag.name} label={tag.name} placement="right" when={collapsed}>
            <button
              type="button"
              className="rail-item"
              // As on a saved search: the row's own menu button is inside it.
              aria-label={tag.name}
              aria-current={current === `tag:${tag.name}` ? 'page' : undefined}
                  onClick={() => onView(`tag:${tag.name}`)}
              onPointerDown={(e) => onDragTag(e, tag.id, tag.name)}
              {...dropTarget(`tag:${tag.name}`, view, dropOver)}
              data-drop-ok={dragActive && acceptsDrop(`tag:${tag.name}`, view) ? true : undefined}
              data-reorder={`tag:${tag.id}`}
              data-insert={insertAt?.key === `tag:${tag.id}` ? insertAt.edge : undefined}
            >
              <span
                className="tag-swatch"
                style={{ background: tag.colour || 'var(--ink3)' }}
                aria-hidden="true"
              />
              <span className="rail-text">{tag.name}</span>
              {/* Before the count, so the two never share a corner. */}
              {!collapsed && (
                <TagMenu
                  name={tag.name}
                  colour={tag.colour}
                  first={at === 0}
                  last={at === tags.length - 1}
                  onReorder={(up) => onReorderRow('tag', tag.id, up)}
                  onRename={() => onAskRename({ kind: 'tag', id: tag.id, name: tag.name })}
                  onColour={(c) => onColourTag(tag.id, c)}
                  onDelete={() => onDeleteTag({ id: tag.id, name: tag.name })}
                />
              )}
              {!collapsed && tag.thread_count > 0 && (
                <span className="count">{tag.thread_count}</span>
              )}
            </button>
            </Tip>
      ))}
    </>
  );

  const searchesSection = (
    <>
      {/* Questions, last by default: the places and labels that hold mail come
          first, and a section nobody has filled yet should not push them down.
          The order is a setting, so this is a default rather than a fact.

          Hidden entirely until the first one is saved. The tags header above
          takes the opposite line and says why, but its reasoning is about the
          `+` it carries: there, the header is the only way to make the first
          tag. A search is saved from the middle pane, from results already on
          screen, so an empty section here would be a heading with nothing to
          offer — and no `+` belongs in it for the same reason. */}
      {searches.length > 0 && (
        <>
          <div className="rail-label" role="heading" aria-level={2}>
            {t('rail-searches')}
          </div>
          {searches.map((s, at) => (
              // Wrapped only while collapsed, which is the whole trick the tag
              // rows use. Ariakit's anchor takes the props of what it wraps, so
              // around this row in an *open* rail it took the drag handler and
              // the row stopped dragging; `when={collapsed}` returns the child
              // untouched, so an open rail is a plain button. Collapsed, the row
              // is all there is to hover — the label inside it is zeroed to no
              // width and no line box, which is why a tooltip on the label
              // never appeared.
              <Tip key={s.id} label={s.name} placement="right" when={collapsed}>
                <button
                  type="button"
                  className="rail-item"
                  // Named explicitly, because the ⋯ menu button sits inside this
                  // one and name-from-contents swallowed its label: every saved
                  // search announced itself as "Waiting on More for Waiting on".
                  aria-label={s.name}
                  // The native tooltip, for the one case Tip cannot cover: a
                  // name too long for the rail is clipped with no way to read
                  // it, and Tip cannot wrap this row — Ariakit's anchor takes
                  // the row's own props, which cost the drag handler once and a
                  // phantom tab stop once. `title` is the exception the rest of
                  // the app avoids, earned by truncation.
                  title={s.name}
                  aria-current={current === `search:${s.id}` ? 'page' : undefined}
                  onClick={() => onView(`search:${s.id}`)}
                  onPointerDown={(e) => onDragSearch(e, s.id, s.name)}
                  data-reorder={`search:${s.id}`}
                  data-insert={insertAt?.key === `search:${s.id}` ? insertAt.edge : undefined}
                  data-dragging={anyDrag || undefined}
                >
                  {/* The magnifier, not a bookmark: the Star two rows above is
                      already the rail's "mark this" glyph, and in a collapsed
                      rail there are no headings to tell the two apart. */}
                  <Icon icon={Search} />
                  {/* Plain. A Tip here would add `tabIndex={0}` to a roleless
                      span inside the row — four dead tab stops, the name read
                      twice — and collapsed it has no size to hover anyway. */}
                  <span className="rail-text">{s.name}</span>
                  {!collapsed && (
                    <SearchMenu
                      name={s.name}
                      first={at === 0}
                      last={at === searches.length - 1}
                      onReorder={(up) => onReorderRow('search', s.id, up)}
                      onRename={() => onAskRename({ kind: 'search', id: s.id, name: s.name })}
                      onDelete={() => onDeleteSearch({ id: s.id, name: s.name })}
                    />
                  )}
                </button>
              </Tip>
          ))}
        </>
      )}
    </>
  );

  /** Each section as one fragment, so the order is the only thing that varies. */
  const sections: Record<SectionKey, React.ReactNode> = {
    mailboxes: mailboxesSection,
    folders: foldersSection,
    tags: tagsSection,
    searches: searchesSection,
  };

  return (
    <nav
      className="rail"
      ref={railRef}
      aria-label={t('rail-mailboxes')}
      data-collapsed={collapsed || undefined}
    >
      {/* One account is active at a time (Q27): the header names it rather than
          leaving "which account am I in" to be inferred. */}
      <AccountMenu
        accounts={accounts}
        current={account}
        // The same number the footer shows for the view on screen. This once
        // preferred the account's stored inbox count, and the two disagreed —
        // a header saying 7 over a pane saying 0 reads as broken, whichever
        // is technically defensible. One view, one number, everywhere it
        // appears; the per-account rows in the menu keep their own counts.
        unread={unread}
        accountColor={accountColor}
        onSwitch={onSwitchAccount}
        onSettings={onSettings}
        onAdd={onAddAccount}
      />

      {/* Writing is the one thing in this rail that is not somewhere to go, so
          it gets the one filled button. C does the same for anyone who has
          learned it — the cap is on the button so they can. */}
      <Tip label={t('cmd-compose')} placement="right" when={collapsed} keys={['C']}>
        <button type="button" className="compose-new" onClick={onCompose}>
          <Icon icon={PenSquare} size={15} />
          <span className="rail-text">{t('cmd-compose')}</span>
          <span className="kbd on-accent rail-text">C</span>
        </button>
      </Tip>

      {/* Everything you navigate to scrolls; the things you reach for do not.
          With a few dozen folders the account switcher, Compose, Help and
          Settings used to scroll off with them, so the way out of a long
          mailbox list was to scroll back up it. */}
      <div className="rail-scroll">
      {/* The sections, in the order Settings holds and only the ones it shows.
          Drawn from a list rather than written out in sequence: the rail used
          to hold its own order, which made "put folders first" a code change.
          Each is a fragment built above, so what is inside one is unchanged by
          where it sits. */}
      {sectionOrder.map((key) => (
        <Fragment key={key}>{sections[key]}</Fragment>
      ))}
      </div>

      {/* One row at the foot of the rail: the two things you go *to* on the
          left, the thing that changes the rail itself on the right. Out of the
          triage path but always in the same place, not hidden behind a menu.

          Icon-only, so the labels are carried by the tooltips and by the
          .rail-text spans, which are still in the DOM for a screen reader —
          dropping them would leave three unnamed buttons, which is the exact
          defect the a11y pass went through 51 tab stops to remove.

          Tooltips are unconditional here, not `when={collapsed}` as they were
          while an expanded rail still showed the words. */}
      <div className="rail-foot">
        <div className="rail-foot-go">
          <Tip label={t('rail-settings')} placement="top">
            <button type="button" className="rail-item" onClick={() => onView('settings')}>
              <Icon icon={Settings} />
              <span className="rail-text">{t('rail-settings')}</span>
            </button>
          </Tip>
          <Tip label={t('rail-help')} placement="top">
            <button type="button" className="rail-item" onClick={() => onView('help')}>
              <Icon icon={CircleHelp} />
              <span className="rail-text">{t('rail-help')}</span>
            </button>
          </Tip>
        </div>
        <Tip label={collapsed ? t('rail-expand') : t('rail-collapse')} placement="top">
          <button
            type="button"
            className="rail-item"
            onClick={onToggleCollapsed}
            aria-expanded={!collapsed}
          >
            <Icon icon={collapsed ? PanelLeftOpen : PanelLeftClose} />
            <span className="rail-text">{t('rail-collapse')}</span>
          </button>
        </Tip>
      </div>

      {/* A separator with a role, not just a draggable strip: resizing by mouse
          only is a common way to lock keyboard users out of their own layout. */}
      {!collapsed && (
        <div
          className="rail-resize"
          role="separator"
          aria-orientation="vertical"
          aria-label={t('rail-resize')}
          tabIndex={0}
          onPointerDown={startDrag}
          onDoubleClick={onToggleCollapsed}
          onKeyDown={(e) => {
            const step = e.shiftKey ? 32 : 8;
            if (e.key === 'ArrowLeft') {
              e.preventDefault();
              onResize(-step);
            } else if (e.key === 'ArrowRight') {
              e.preventDefault();
              onResize(step);
            }
          }}
        />
      )}

      <NameDialog
        open={namingDialog === 'folder'}
        title={t('folder-new')}
        placeholder={t('folder-new-placeholder')}
        icon={FolderPlus}
        confirmLabel={t('create')}
        onClose={() => setNamingDialog(null)}
        onSubmit={(name) => void onCreateFolder(name)}
      />
      <NameDialog
        open={namingDialog === 'tag'}
        title={t('tag-new')}
        placeholder={t('tag-new-placeholder')}
        icon={TagPlus}
        confirmLabel={t('create')}
        onClose={() => setNamingDialog(null)}
        onSubmit={(name) => void onCreateTag(name)}
      />
    </nav>
  );
}
