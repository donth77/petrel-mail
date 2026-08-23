import type React from 'react';
import { useEffect, useRef, useState } from 'react';
import {
  Inbox, Star, Clock, Send, PencilLine, Upload, Archive, ShieldAlert, Trash2,
  CircleHelp, PanelLeftClose, PanelLeftOpen, PenSquare, Plus, Settings, type LucideIcon,
} from 'lucide-react';
import type { Account } from '../lib/api';
import { Icon } from './Icon';
import { t, type StringId } from '../lib/strings';
import { DRAG_TYPE, acceptsDrop } from '../lib/dnd';
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

type Tag = { name: string; colour: string; thread_count: number };

/**
 * The drop handlers a rail destination needs.
 *
 * Returned as a bundle rather than written out at each call site because a
 * destination that lights up but does not accept, or accepts but never lit up,
 * is the confusing half-state this avoids by construction.
 *
 * `dragover` must cancel the event or the browser refuses the drop — the
 * default action for a dragged item is "no". `dragleave` fires when the pointer
 * crosses into a child element too, so the highlight is keyed on the row that
 * owns it rather than toggled blindly.
 */
function dropProps(
  railKey: string,
  view: string,
  dragActive: boolean,
  over: string | null,
  setOver: React.Dispatch<React.SetStateAction<string | null>>,
  onDrop: (railKey: string, ids: number[]) => void,
) {
  if (!acceptsDrop(railKey, view)) return {};
  return {
    // Marked while anything is being dragged, not only once the pointer is
    // here: a destination that reveals itself only on arrival is one you have
    // to already know about to find.
    'data-drop-ok': dragActive || undefined,
    'data-drop-over': over === railKey || undefined,
    onDragOver: (e: React.DragEvent) => {
      if (!e.dataTransfer.types.includes(DRAG_TYPE)) return;
      e.preventDefault();
      e.dataTransfer.dropEffect = 'move';
      setOver(railKey);
    },
    onDragLeave: (e: React.DragEvent) => {
      if (e.currentTarget.contains(e.relatedTarget as Node | null)) return;
      // Cleared only if this row is still the one lit. Leaving one row can
      // arrive after entering the next, and an unconditional clear would put
      // the highlight out on the row the pointer just moved onto.
      setOver((cur) => (cur === railKey ? null : cur));
    },
    onDrop: (e: React.DragEvent) => {
      const raw = e.dataTransfer.getData(DRAG_TYPE);
      if (!raw) return;
      e.preventDefault();
      setOver(null);
      try {
        const ids: unknown = JSON.parse(raw);
        if (Array.isArray(ids) && ids.every((n) => typeof n === 'number')) {
          onDrop(railKey, ids as number[]);
        }
      } catch {
        // A drag from somewhere else wearing our type. Nothing to do.
      }
    },
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
  /** Conversations dropped on a destination. The rail decides where; what that
      means to the store is the caller's business. */
  onDropThreads: (railKey: string, ids: number[]) => void;
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
  onView: (v: string) => void;
  /** Make a tag that is attached to nothing yet. Returns once it exists, so the
   *  rail can put the input away only after the work succeeded. */
  onCreateTag: (name: string) => Promise<void>;
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
  collapsed,
  onView,
  onCreateTag,
  onToggleCollapsed,
  onCompose,
  onResize,
  onSwitchAccount,
  onSettings,
  onDropThreads,
  dragActive,
  railRef,
}: Props) {
  // Which destination the pointer is over mid-drag, for the highlight.
  const [dropOver, setDropOver] = useState<string | null>(null);
  // Pointer drag, with the listeners on the window rather than the handle: a
  // fast drag outruns a 6px target, and losing the pointer mid-resize leaves
  // the rail stuck at whatever width the last event happened to land on.
  // Naming a new tag. An inline field rather than a dialog: it is one short
  // string, and a modal for one word is more ceremony than the act deserves.
  const [naming, setNaming] = useState(false);
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
            onClick={() => onView(m.key)}
            {...dropProps(m.key, view, dragActive, dropOver, setDropOver, onDropThreads)}
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

      {/* The header shows even with no tags yet, because the + is how the first
          one gets made — a section that only appears once you already have one
          is a feature you cannot find. Collapsed there is no header row to put
          a button in, so it goes; the tag rows themselves stay, because they
          are still somewhere to go. */}
      {!collapsed && (
        <>
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
          {naming && (
            <input
              ref={nameInput}
              className="rail-new-tag"
              placeholder={t('tag-new-placeholder')}
              aria-label={t('tag-new')}
              autoComplete="off"
              onBlur={() => setNaming(false)}
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
                void onCreateTag(name).then(() => setNaming(false));
              }}
            />
          )}
        </>
      )}

      {tags.map((tag) => (
            <Tip key={tag.name} label={tag.name} placement="right" when={collapsed}>
            <button
              type="button"
              className="rail-item"
              aria-current={view === `tag:${tag.name}` ? 'page' : undefined}
              onClick={() => onView(`tag:${tag.name}`)}
              {...dropProps(`tag:${tag.name}`, view, dragActive, dropOver, setDropOver, onDropThreads)}
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
            </button>
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
