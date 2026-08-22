import {
  Inbox, Star, Clock, Send, PencilLine, Upload, Archive, ShieldAlert, Trash2,
  CircleHelp, PanelLeftClose, PanelLeftOpen, PenSquare, Settings, type LucideIcon,
} from 'lucide-react';
import type { Account } from '../lib/api';
import { Icon } from './Icon';
import { t, type StringId } from '../lib/strings';
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
  accountColor: string;
  unread: number;
  /** Per-mailbox numbers, keyed by rail key. Absent means nothing to show —
   *  the engine omits empty ones rather than sending zeroes. */
  counts: Record<string, number>;
  view: string;
  tags: Tag[];
  onView: (v: string) => void;
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
  onToggleCollapsed,
  onCompose,
  onResize,
  onSwitchAccount,
  onSettings,
  railRef,
}: Props) {
  // Pointer drag, with the listeners on the window rather than the handle: a
  // fast drag outruns a 6px target, and losing the pointer mid-resize leaves
  // the rail stuck at whatever width the last event happened to land on.
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
        unread={unread}
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

      {tags.length > 0 && (
        <>
          <div className="rail-label">{t('rail-tags')}</div>
          {tags.map((tag) => (
            <Tip key={tag.name} label={tag.name} placement="right" when={collapsed}>
            <button
              type="button"
              className="rail-item"
              aria-current={view === `tag:${tag.name}` ? 'page' : undefined}
              onClick={() => onView(`tag:${tag.name}`)}
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
        </>
      )}

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
