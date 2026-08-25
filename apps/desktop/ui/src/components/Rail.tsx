import type React from 'react';
import { useEffect, useRef, useState } from 'react';
import {
  ChevronDown, ChevronRight, FolderClosed, Inbox, Star, Clock, Send, PencilLine, Upload, Archive, ShieldAlert, Trash2,
  CircleHelp, PanelLeftClose, PanelLeftOpen, PenSquare, Plus, Settings, type LucideIcon,
} from 'lucide-react';
import type { Account, Folder } from '../lib/api';
import { Icon } from './Icon';
import { t, type StringId } from '../lib/strings';
import { TagMenu } from './TagMenu';
import { FolderMenu } from './FolderMenu';
import { acceptsDrop } from '../lib/dnd';
import { AccountMenu } from './AccountMenu';
import { Tip } from './Tip';

const MAILBOXES: { id: StringId; key: string; glyph: LucideIcon }[] = [
  { id: 'mailbox-inbox', key: 'inbox', glyph: Inbox },
  { id: 'mailbox-starred', key: 'starred', glyph: Star },
  { id: 'mailbox-snoozed', key: 'snoozed', glyph: Clock },
  { id: 'mailbox-sent', key: 'sent', glyph: Send },
  { id: 'mailbox-drafts', key: 'drafts', glyph: PencilLine },
  { id: 'mailbox-outbox', key: 'outbox', glyph: Upload },
  { id: 'mailbox-archive', key: 'archive', glyph: Archive },
  { id: 'mailbox-spam', key: 'spam', glyph: ShieldAlert },
  { id: 'mailbox-trash', key: 'trash', glyph: Trash2 },
];

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
  /** Outbox messages waiting on a decision. Any at all turns the row amber:
      a message that needs a person must not go unnoticed, and this is where
      you find out — the sidebar, not a dialog. */
  outboxNeedsAttention: number;
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
  dragActive,
  outboxNeedsAttention,
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
  /** Paths whose children are folded away. */
  const [closedNodes, setClosedNodes] = useState<Set<string>>(new Set());
  const [renamingFolder, setRenamingFolder] = useState<number | null>(null);
  // The tag being renamed, edited in place on its own row rather than in a
  // dialog: it is one short string, and the row is where you are looking.
  const [renaming, setRenaming] = useState<number | null>(null);
  /** Where the archive tree roots, for the mailbox row's folder-drop. */
  const archiveRolePath = folders.find((f) => f.role === 'archive')?.path;
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
        // The account's own unread, not the loaded page's. Deriving it from the
        // rows in view made the header report Trash's unread count while
        // sitting in Trash, which is not a fact about the account at all. This
        // number also ignores the badge setting: that governs the numbers
        // beside the mailboxes, not whether the account can say how it is.
        unread={accounts.find((a) => a.email === account)?.unread_count ?? unread}
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

      <div className="rail-label">{t('rail-mailboxes')}</div>
      {MAILBOXES.map((m) => (
        <Tip key={m.key} label={t(m.id)} placement="right" when={collapsed}>
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
            data-drop-over={
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
            <Icon icon={m.glyph} />
            <span className="rail-text">{t(m.id)}</span>
            {/* Collapsed, there is no room for a number beside a 16px icon,
                and a dot that only says "something" is not worth the pixels —
                the tooltip carries the label, and expanding carries the count. */}
            {!collapsed && counts[m.key] > 0 && (
              <span className="count">{counts[m.key]}</span>
            )}
          </button>
        </Tip>
      ))}


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
            onClick={() => setNamingFolder(true)}
          >
            <Icon icon={Plus} size={13} />
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
      {(() => {
        /* The hierarchy the paths already spell, drawn as one. A mailbox
           tree like Archive/Yearly/2023 is how people actually file, and a
           flat list of leaf names turned forty filed years into anonymous
           siblings. Containers that are not themselves folders (and the
           Archive root, which is the archive mailbox wearing its tree) still
           get a row, for the chevron. */
        type FNode = {
          label: string;
          path: string;
          folder?: (typeof folders)[number];
          archiveRoot?: boolean;
          children: FNode[];
        };
        const archivePath = folders.find((f) => f.role === 'archive')?.path;
        const roots: FNode[] = [];
        const attach = (path: string): FNode => {
          const segs = path.split(/[/.]/);
          let level = roots;
          let prefix = '';
          let node: FNode | undefined;
          for (const [si, seg] of segs.entries()) {
            prefix = si === 0 ? seg : `${prefix}${path[prefix.length]}${seg}`;
            node = level.find((n) => n.path === prefix);
            if (!node) {
              node = { label: seg, path: prefix, children: [] };
              if (archivePath && prefix === archivePath) node.archiveRoot = true;
              level.push(node);
              level.sort((a, b) => a.label.localeCompare(b.label));
            }
            level = node.children;
          }
          return node!;
        };
        for (const f of folders.filter((x) => !x.role)) {
          attach(f.path).folder = f;
        }
        // Roots that exist only because the archive tree hangs off them stay;
        // any other childless container is noise from a stale path.
        const prune = (ns: FNode[]): FNode[] =>
          ns
            .map((n) => ({ ...n, children: prune(n.children) }))
            .filter((n) => n.folder || n.archiveRoot || n.children.length > 0);
        const tree = prune(roots);

        const toggle = (path: string) =>
          setClosedNodes((prev) => {
            const next = new Set(prev);
            if (next.has(path)) next.delete(path);
            else next.add(path);
            return next;
          });

        const renderNode = (n: FNode, depth: number): React.ReactNode => {
          const open = !closedNodes.has(n.path);
          const chevron = n.children.length > 0 && !collapsed && (
            <button
              type="button"
              className="tree-toggle"
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
          const indent = { paddingLeft: 10 + depth * 14 } as const;
          const f = n.folder;
          const row = f ? (
            <Tip key={f.id} label={f.path} placement="right" when={collapsed}>
              {renamingFolder === f.id ? (
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
                  onClick={() => onView(`folder:${f.id}`)}
                  onPointerDown={(e) => onDragFolder(e, f.id, n.label)}
                  data-folder-drop={f.path}
                  data-drop-over={dropOver === `fdrop:${f.path}` || undefined}
                  {...dropTarget(`folder:${f.id}`, view, dropOver)}
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
                  {!collapsed && counts[`folder:${f.id}`] > 0 && (
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
                      onArchiveInto={
                        archivePath && !f.path.startsWith(`${archivePath}/`)
                          ? () => void onRenameFolder(f.id, `${archivePath}/${n.label}`)
                          : undefined
                      }
                      onUnarchive={
                        archivePath && f.path.startsWith(`${archivePath}/`)
                          ? () => void onRenameFolder(f.id, n.label)
                          : undefined
                      }
                      onDelete={() => onDeleteFolder(f)}
                    />
                  )}
                </button>
              )}
            </Tip>
          ) : (
            <button
              key={n.path}
              type="button"
              className="rail-item tree-container"
              style={indent}
              aria-current={n.archiveRoot && view === 'archive' ? 'page' : undefined}
              onClick={() => (n.archiveRoot ? onView('archive') : toggle(n.path))}
              data-folder-drop={n.archiveRoot ? n.path : undefined}
              data-drop-over={
                (n.archiveRoot && dropOver === `fdrop:${n.path}`) || undefined
              }
              data-drop-ok={(n.archiveRoot && folderDragPath !== null) || undefined}
            >
              {chevron}
              <Icon icon={n.archiveRoot ? Archive : FolderClosed} />
              <span className="rail-text">{n.label}</span>
              {!collapsed && n.archiveRoot && (
                <FolderMenu
                  path={n.path}
                  onNewChild={() => {
                    setFolderPrefill(`${n.path}/`);
                    setNamingFolder(true);
                  }}
                />
              )}
            </button>
          );
          return (
            <div key={n.path}>
              {row}
              {open && n.children.map((c) => renderNode(c, depth + 1))}
            </div>
          );
        };
        return tree.map((n) => renderNode(n, 0));
      })()}
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
                onClick={() => setNaming(true)}
              >
                <Icon icon={Plus} size={13} />
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
              onClick={() => onView(`tag:${tag.name}`)}
              onPointerDown={(e) => onDragTag(e, tag.id, tag.name)}
              {...dropTarget(`tag:${tag.name}`, view, dropOver)}
              data-drop-ok={dragActive && acceptsDrop(`tag:${tag.name}`, view) ? true : undefined}
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

      {/* Help and Settings sit at the foot of the rail, out of the triage path
          but always in the same place — not hidden behind a menu. */}
      <div className="rail-foot">
        <Tip label={t('rail-help')} placement="right" when={collapsed}>
          <button type="button" className="rail-item" onClick={() => onView('help')}>
            <Icon icon={CircleHelp} />
            <span className="rail-text">{t('rail-help')}</span>
          </button>
        </Tip>
        <Tip label={t('rail-settings')} placement="right" when={collapsed}>
          <button type="button" className="rail-item" onClick={() => onView('settings')}>
            <Icon icon={Settings} />
            <span className="rail-text">{t('rail-settings')}</span>
          </button>
        </Tip>
        <Tip
          label={collapsed ? t('rail-expand') : t('rail-collapse')}
          placement="right"
          when={collapsed}
        >
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
    </nav>
  );
}
