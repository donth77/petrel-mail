import type React from 'react';
import { useEffect, useRef, useState } from 'react';
import {
  ChevronDown, ChevronRight, FolderClosed, Inbox, Star, Clock, Send, PencilLine, Upload, Archive, ShieldAlert, Trash2,
  CircleHelp, PanelLeftClose, PanelLeftOpen, PenSquare, Plus, Settings, type LucideIcon, FolderPlus, TagPlus } from 'lucide-react';
import type { Account, Folder } from '../lib/api';
import { Icon } from './Icon';
import { t, type StringId } from '../lib/strings';
import { TagMenu } from './TagMenu';
import { FolderMenu } from './FolderMenu';
import { NameDialog } from './NameDialog';
import { acceptsDrop } from '../lib/dnd';
import type { InsertPoint } from '../lib/useDrag';
import { buildFolderTree, type FolderNode, nestableRolePath, underAnchor } from '../lib/folders';
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
  /** Outbox messages waiting on a decision. Any at all turns the row amber:
      a message that needs a person must not go unnoticed, and this is where
      you find out — the sidebar, not a dialog. */
  outboxNeedsAttention: number;
  /** Which mailboxes to draw, in order. From the sidebar arrangement, so a row
   *  somebody hid is simply absent rather than drawn and ignored. */
  mailboxOrder: string[];
  /** Whether a drag is in flight, so destinations can say they will take it
      before the pointer reaches them rather than only once it arrives. */
  dragActive: boolean;
  accountColor: string;
  unread: number;
  /** Per-mailbox numbers, keyed by rail key. Absent means nothing to show —
   *  the engine omits empty ones rather than sending zeroes. */
  counts: Record<string, number>;
  view: string;
  tags: Tag[];
  /** Every folder; the rail lists the ones the user made (no role). */
  folders: Folder[];
  onView: (v: string) => void;
  onCreateFolder: (name: string) => Promise<void>;
  /** Begins carrying a folder toward a new parent. */
  onDragFolder: (e: React.PointerEvent, folderId: number, label: string) => void;
  /** Path of the folder mid-drag, so valid destinations can say so — and so
   *  the folder itself and its descendants can decline to light up. */
  folderDragPath: string | null;
  onRenameFolder: (folderId: number, newPath: string) => Promise<void>;
  onDeleteFolder: (folder: Folder) => void;
  /** Opens the move-destination picker for this folder. */
  onMoveFolder: (folder: Folder) => void;
  /** Asks to empty the bin. Absent in windows that do not own that. */
  onEmptyTrash?: () => void;
  /** Make a tag that is attached to nothing yet. Returns once it exists, so the
   *  rail can put the input away only after the work succeeded. */
  onCreateTag: (name: string) => Promise<void>;
  onRenameTag: (tagId: number, name: string) => Promise<void>;
  onColourTag: (tagId: number, colour: string) => void;
  onDeleteTag: (tag: { id: number; name: string }) => void;
  /** Begins carrying this tag towards a conversation. */
  onDragTag: (e: React.PointerEvent, tagId: number, name: string) => void;
  railRef?: React.Ref<HTMLElement>;
};

export function Rail({
  account,
  accounts,
  accountColor,
  unread,
  counts,
  view,
  tags,
  folders,
  collapsed,
  onView,
  onCreateFolder,
  onDragFolder,
  folderDragPath,
  onRenameFolder,
  onDeleteFolder,
  onMoveFolder,
  onEmptyTrash,
  onCreateTag,
  onRenameTag,
  onColourTag,
  onDeleteTag,
  onDragTag,
  onToggleCollapsed,
  onCompose,
  onResize,
  onSwitchAccount,
  onSettings,
  onAddAccount,
  dropOver,
  insertAt,
  dragActive,
  outboxNeedsAttention,
  mailboxOrder,
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
   *  absent takes the default — see foldedByDefault. */
  const [folded, setFolded] = useState<Record<string, boolean>>({});
  const [renamingFolder, setRenamingFolder] = useState<number | null>(null);
  // Which naming dialog is up — the collapsed rail's way of asking for a
  // name without forcing itself open.
  const [namingDialog, setNamingDialog] = useState<'folder' | 'tag' | null>(null);
  // The tag being renamed, edited in place on its own row rather than in a
  // dialog: it is one short string, and the row is where you are looking.
  const [renaming, setRenaming] = useState<number | null>(null);
  /** Where the archive tree roots, for the mailbox row's folder-drop. */
  const archiveRolePath = nestableRolePath(folders, 'archive');
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
  const tree = buildFolderTree(
    own.filter((f) => !underAnchor(f.path, archivePath) && !underAnchor(f.path, trashPath)),
  );
  const archiveTree = buildFolderTree(
    own.filter((f) => under(f, archivePath)),
    archivePath?.length ?? 0,
  );
  const trashTree = buildFolderTree(
    own.filter((f) => under(f, trashPath)),
    trashPath?.length ?? 0,
  );
  /* Archive and Trash start folded. What hangs off them is mail already dealt
     with, and a rail that opens with forty archived years unrolled pushes the
     folders you actually work in off the bottom of the screen. Only the two
     anchors default this way: once one is open, the rows inside it fold and
     unfold like every other row. */
  const foldedByDefault = (path: string) => path === archivePath || path === trashPath;
  const isOpen = (path: string) => !(folded[path] ?? foldedByDefault(path));
  const archiveOpen = archivePath !== undefined && isOpen(archivePath);
  const trashOpen = trashPath !== undefined && isOpen(trashPath);

  const toggle = (path: string) =>
    setFolded((prev) => ({ ...prev, [path]: !(prev[path] ?? foldedByDefault(path)) }));

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
  const renderNode = (n: FNode, depth: number, owner?: string): React.ReactNode => {
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
      renamingFolder === f.id ? (
        <input
          key={`rename-folder-${f.id}`}
          className="rail-new-tag"
          defaultValue={f.path}
          aria-label={t('folder-rename')}
          autoComplete="off"
          autoFocus
          onFocus={(e) => e.currentTarget.select()}
          onBlur={(e) => {
            const next = e.currentTarget.value.trim();
            setRenamingFolder(null);
            if (next && next !== f.path) void onRenameFolder(f.id, next);
          }}
          onKeyDown={(e) => {
            e.stopPropagation();
            if (e.key === 'Escape') {
              e.currentTarget.value = f.path;
              setRenamingFolder(null);
              return;
            }
            if (e.key === 'Enter') e.currentTarget.blur();
          }}
        />
      ) : (
        <button
          type="button"
          className="rail-item"
          style={indent}
          aria-current={view === `folder:${f.id}` ? 'page' : undefined}
          data-has-menu={!collapsed ? true : undefined}
          onClick={() => onView(`folder:${f.id}`)}
          onPointerDown={(e) => {
            setDragOrigin(owner ?? null);
            onDragFolder(e, f.id, n.label);
          }}
          {...dropTarget(`folder:${f.id}`, view, dropOver)}
          data-folder-drop={f.path}
          data-reorder={f.id}
          // Which edge to draw the line against. CSS puts it there; keeping
          // the decision in one attribute means the line cannot appear on
          // two rows at once.
          data-insert={insertAt?.key === String(f.id) ? insertAt.edge : undefined}
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
          {!dense && counts[`folder:${f.id}`] > 0 && (
            <span className="count">{counts[`folder:${f.id}`]}</span>
          )}
          {!collapsed && (
            <FolderMenu
              path={f.path}
              onRename={() => setRenamingFolder(f.id)}
              onNewChild={() => {
                setFolderPrefill(`${f.path}/`);
                setNamingFolder(true);
              }}
              onMove={() => onMoveFolder(f)}
              onDelete={() => onDeleteFolder(f)}
            />
          )}
        </button>
      )
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
          {n.children.map((c) => renderNode(c, 0, card))}
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
        {open && n.children.map((c) => renderNode(c, depth + 1, owner))}
      </div>
    );
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
      <div className="rail-label">{t('rail-mailboxes')}</div>
      {mailboxOrder
        .map((key) => MAILBOXES.find((m) => m.key === key))
        .filter((m): m is (typeof MAILBOXES)[number] => m !== undefined)
        .map((m) => {
        const subtree = m.key === 'archive' ? archiveTree : m.key === 'trash' ? trashTree : [];
        const anchor = (
          <button
            type="button"
            className="rail-item"
            aria-current={view === m.key ? 'page' : undefined}
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
            {!collapsed && counts[m.key] > 0 && (
              <span className="count">{counts[m.key]}</span>
            )}
            {/* Archived folders hang off this row, so this row also carries
                their chevron and the way to make the first one. */}
            {m.key === 'archive' && !collapsed && archiveTree.length > 0 && archivePath && (
              <FolderMenu
                path={archivePath}
                onNewChild={() => {
                  setFolderPrefill(`${archivePath}/`);
                  setNamingFolder(true);
                }}
              />
            )}
            {/* The bin's verb lives where every other folder verb lives,
                rather than in the list header: it is a thing done to a
                folder, and the header has no other actions for it to sit
                beside. Always offered, not only when the bin holds folders —
                a menu that appears and disappears is one nobody learns. */}
            {m.key === 'trash' && !collapsed && onEmptyTrash && (
              <FolderMenu path={trashPath ?? 'Trash'} onEmpty={onEmptyTrash} />
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
              {subtree.map((c) => renderNode(c, 0, `mailbox:${m.key}`))}
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
              {!collapsed && archiveOpen && archiveTree.map((c) => renderNode(c, 1))}
            </div>
          );
        }
        if (m.key === 'trash' && trashTree.length > 0) {
          return (
            <div key={m.key}>
              {row}
              {!collapsed && trashOpen && trashTree.map((c) => renderNode(c, 1))}
            </div>
          );
        }
        return row;
        })}


      {/* Folders the user made, between the fixed mailboxes and the tags —
          places before labels. The header shows even with none yet, because
          the + is how the first one gets made. */}
      <div
        className="rail-label rail-label-row"
        data-folder-drop=""
        data-drop-over={dropOver === 'fdrop:' || undefined}
        data-drop-ok={folderDragPath !== null || undefined}
      >
        <span>{t('rail-folders')}</span>
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
      {tree.map((n) => renderNode(n, 0))}
      {/* The header shows even with no tags yet, because the + is how the first
          one gets made — a section that only appears once you already have one
          is a feature you cannot find.

          It stays in the layout when the rail collapses, hidden the same way
          the Mailboxes heading is. Removing it took its 37px with it and every
          tag below jumped up, while the mailboxes — whose heading only fades —
          held still. Two headings, two behaviours, one of them visibly wrong. */}
      <div className="rail-label rail-label-row">
            <span>{t('rail-tags')}</span>
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

      {tags.map((tag) => (
            <Tip key={tag.name} label={tag.name} placement="right" when={collapsed}>
            {renaming === tag.id ? (
              <input
                key={`rename-${tag.id}`}
                className="rail-new-tag"
                defaultValue={tag.name}
                aria-label={t('tag-rename')}
                autoComplete="off"
                autoFocus
                onFocus={(e) => e.currentTarget.select()}
                onBlur={(e) => {
                  const next = e.currentTarget.value.trim();
                  setRenaming(null);
                  if (next && next !== tag.name) void onRenameTag(tag.id, next);
                }}
                onKeyDown={(e) => {
                  e.stopPropagation();
                  if (e.key === 'Escape') {
                    // Abandoned, so blur must not then commit it.
                    e.currentTarget.value = tag.name;
                    setRenaming(null);
                    return;
                  }
                  if (e.key === 'Enter') e.currentTarget.blur();
                }}
              />
            ) : (
            <button
              type="button"
              className="rail-item"
              aria-current={view === `tag:${tag.name}` ? 'page' : undefined}
              data-has-menu={!collapsed ? true : undefined}
              onClick={() => onView(`tag:${tag.name}`)}
              onPointerDown={(e) => onDragTag(e, tag.id, tag.name)}
              {...dropTarget(`tag:${tag.name}`, view, dropOver)}
              data-drop-ok={dragActive && acceptsDrop(`tag:${tag.name}`, view) ? true : undefined}
              data-reorder={tag.id}
              data-insert={insertAt?.key === String(tag.id) ? insertAt.edge : undefined}
            >
              <span
                className="tag-swatch"
                style={{ background: tag.colour || 'var(--ink3)' }}
                aria-hidden="true"
              />
              <span className="rail-text">{tag.name}</span>
              {!collapsed && tag.thread_count > 0 && (
                <span className="count">{tag.thread_count}</span>
              )}
              {!collapsed && (
                <TagMenu
                  name={tag.name}
                  colour={tag.colour}
                  onRename={() => setRenaming(tag.id)}
                  onColour={(c) => onColourTag(tag.id, c)}
                  onDelete={() => onDeleteTag({ id: tag.id, name: tag.name })}
                />
              )}
            </button>
            )}
            </Tip>
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
        onClose={() => setNamingDialog(null)}
        onSubmit={(name) => void onCreateFolder(name)}
      />
      <NameDialog
        open={namingDialog === 'tag'}
        title={t('tag-new')}
        placeholder={t('tag-new-placeholder')}
        icon={TagPlus}
        onClose={() => setNamingDialog(null)}
        onSubmit={(name) => void onCreateTag(name)}
      />
    </nav>
  );
}
