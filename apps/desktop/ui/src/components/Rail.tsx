import {
  Inbox, Star, Clock, Send, PencilLine, Upload, Archive, ShieldAlert, Trash2,
  CircleHelp, PanelLeftClose, PanelLeftOpen, Settings, type LucideIcon,
} from 'lucide-react';
import type { Account } from '../lib/api';
import { Icon } from './Icon';
import { t, type StringId } from '../lib/strings';
import { AccountMenu } from './AccountMenu';
import { RailTip } from './RailTip';

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
  /** Absolute x during a drag, or a signed delta from the keyboard. */
  onResize: (xOrDelta: number) => void;
  onSwitchAccount: (index: number) => void;
  onSettings: () => void;
  onNotImplemented: (label: string) => void;
  accountColor: string;
  unread: number;
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
  view,
  tags,
  collapsed,
  onView,
  onToggleCollapsed,
  onResize,
  onSwitchAccount,
  onSettings,
  onNotImplemented,
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
        onNotImplemented={onNotImplemented}
      />

      <div className="rail-label">{t('rail-mailboxes')}</div>
      {MAILBOXES.map((m) => (
        <RailTip key={m.key} label={t(m.id)} collapsed={collapsed}>
          <button
            type="button"
            className="rail-item"
            aria-current={view === m.key ? 'page' : undefined}
            onClick={() => onView(m.key)}
          >
            <Icon icon={m.glyph} />
            <span className="rail-text">{t(m.id)}</span>
            {m.key === 'inbox' && unread > 0 && <span className="count">{unread}</span>}
          </button>
        </RailTip>
      ))}

      {tags.length > 0 && (
        <>
          <div className="rail-label">{t('rail-tags')}</div>
          {tags.map((tag) => (
            <RailTip key={tag.name} label={tag.name} collapsed={collapsed}>
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
              {tag.thread_count > 0 && <span className="count">{tag.thread_count}</span>}
            </button>
            </RailTip>
          ))}
        </>
      )}

      {/* Help and Settings sit at the foot of the rail, out of the triage path
          but always in the same place — not hidden behind a menu. */}
      <div className="rail-foot">
        <RailTip label={t('rail-help')} collapsed={collapsed}>
          <button type="button" className="rail-item" onClick={() => onView('help')}>
            <Icon icon={CircleHelp} />
            <span className="rail-text">{t('rail-help')}</span>
          </button>
        </RailTip>
        <RailTip label={t('rail-settings')} collapsed={collapsed}>
          <button type="button" className="rail-item" onClick={() => onView('settings')}>
            <Icon icon={Settings} />
            <span className="rail-text">{t('rail-settings')}</span>
          </button>
        </RailTip>
        {/* The toggle keeps a tooltip when expanded too: it is the one item
            whose icon does not say which way it goes. */}
        <RailTip label={collapsed ? t('rail-expand') : t('rail-collapse')} collapsed>
          <button
            type="button"
            className="rail-item"
            onClick={onToggleCollapsed}
            aria-expanded={!collapsed}
          >
            <Icon icon={collapsed ? PanelLeftOpen : PanelLeftClose} />
            <span className="rail-text">{t('rail-collapse')}</span>
          </button>
        </RailTip>
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
