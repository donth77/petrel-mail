import {
  Inbox, Star, Clock, Send, PencilLine, Upload, Archive, ShieldAlert, Trash2, ChevronDown,
  type LucideIcon,
} from 'lucide-react';
import { Icon } from './Icon';
import { t, type StringId } from '../lib/strings';

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

type Props = { account: string; unread: number; view: string; onView: (v: string) => void };

export function Rail({ account, unread, view, onView }: Props) {
  return (
    <nav className="rail" aria-label={t('rail-mailboxes')}>
      {/* One account is active at a time (Q27): the header names it rather than
          leaving "which account am I in" to be inferred. */}
      <button className="account" type="button">
        <span className="dot" style={{ background: 'var(--accent)' }} />
        <span style={{ minInlineSize: 0, flexGrow: 1 }}>
          <span className="clip" style={{ display: 'block', fontSize: 12.5, fontWeight: 600 }}>
            {account}
          </span>
          <span className="mono" style={{ fontSize: 10, color: 'var(--ink3)' }}>
            {t('list-unread', { count: unread })}
          </span>
        </span>
        <Icon icon={ChevronDown} size={13} />
      </button>

      <div className="rail-label">{t('rail-mailboxes')}</div>
      {MAILBOXES.map((m) => (
        <button
          key={m.key}
          type="button"
          className="rail-item"
          aria-current={view === m.key ? 'page' : undefined}
          onClick={() => onView(m.key)}
        >
          <Icon icon={m.glyph} />
          {t(m.id)}
          {m.key === 'inbox' && unread > 0 && <span className="count">{unread}</span>}
        </button>
      ))}
    </nav>
  );
}
