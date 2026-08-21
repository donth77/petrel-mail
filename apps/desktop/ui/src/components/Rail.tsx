import {
  Inbox, Star, Clock, Send, PencilLine, Upload, Archive, ShieldAlert, Trash2, ChevronDown,
  CircleHelp, Settings, type LucideIcon,
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

type Tag = { name: string; colour: string; thread_count: number };

type Props = {
  account: string;
  accountColor: string;
  unread: number;
  view: string;
  tags: Tag[];
  onView: (v: string) => void;
  railRef?: React.Ref<HTMLElement>;
};

export function Rail({ account, accountColor, unread, view, tags, onView, railRef }: Props) {
  return (
    <nav className="rail" ref={railRef} aria-label={t('rail-mailboxes')}>
      {/* One account is active at a time (Q27): the header names it rather than
          leaving "which account am I in" to be inferred. */}
      <button className="account" type="button">
        {/* The account's own colour, not the app accent — the whole point of
            setting one is telling accounts apart at a glance. */}
        <span className="dot" style={{ background: accountColor }} />
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

      {tags.length > 0 && (
        <>
          <div className="rail-label">{t('rail-tags')}</div>
          {tags.map((tag) => (
            <button
              key={tag.name}
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
              {tag.name}
              {tag.thread_count > 0 && <span className="count">{tag.thread_count}</span>}
            </button>
          ))}
        </>
      )}

      {/* Help and Settings sit at the foot of the rail, out of the triage path
          but always in the same place — not hidden behind a menu. */}
      <div className="rail-foot">
        <button type="button" className="rail-item" onClick={() => onView('help')}>
          <Icon icon={CircleHelp} />
          {t('rail-help')}
        </button>
        <button type="button" className="rail-item" onClick={() => onView('settings')}>
          <Icon icon={Settings} />
          {t('rail-settings')}
        </button>
      </div>
    </nav>
  );
}
