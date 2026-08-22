import { useEffect, useRef, useState } from 'react';
import {
  Archive,
  ChevronDown,
  CornerUpLeft,
  Mail,
  MailOpen,
  Paperclip,
  Star,
} from 'lucide-react';
import { api, type ActionKind, type Thread, type ThreadMessage } from '../lib/api';
import { count as fmtCount, fileSize, fullTime, initials, messageTime } from '../lib/format';
import { Icon } from './Icon';
import { MessageBody } from './MessageBody';
import { MoreMenu } from './MoreMenu';
import { Tip } from './Tip';
import { key } from '../lib/keys';
import { t } from '../lib/strings';

/** A message that is not the one you came here to read: one line, expandable. */
function Collapsed({ m, onExpand }: { m: ThreadMessage; onExpand: () => void }) {
  return (
    <button type="button" className="collapsed" onClick={onExpand}>
      <span className="avatar sm" aria-hidden="true">
        {initials(m.from_display, m.from_addr)}
      </span>
      <span className="collapsed-from">{m.from_display || m.from_addr}</span>
      <span className="collapsed-snip clip">{m.snippet}</span>
      <Tip label={fullTime(m.date_ms)}>
        <time className="mono collapsed-time" dateTime={new Date(m.date_ms).toISOString()}>
          {messageTime(m.date_ms)}
        </time>
      </Tip>
    </button>
  );
}

function Expanded({
  m,
  focused,
  onCollapse,
}: {
  m: ThreadMessage;
  focused: boolean;
  onCollapse: () => void;
}) {
  return (
    <article className="msg" id={`msg-body-${m.id}`} data-focused={focused || undefined}>
      {/* The header is the toggle, the way it is in every mail client: if
          clicking a collapsed message opens it, clicking the open one has to
          close it again, or the only way back is to leave the conversation.
          A button rather than a click handler on the header, so it is
          reachable from the keyboard and announces itself as expanded. */}
      <header
        className="msg-head"
        role="button"
        tabIndex={0}
        aria-expanded="true"
        aria-label={t('reader-collapse', { who: m.from_display || m.from_addr })}
        onClick={onCollapse}
        onKeyDown={(e) => {
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault();
            onCollapse();
          }
        }}
      >
        <span className="avatar" aria-hidden="true">
          {initials(m.from_display, m.from_addr)}
        </span>
        <span className="msg-who">
          <span className="msg-name">{m.from_display || m.from_addr}</span>
          <span className="msg-to">
            {m.recipients.length > 0 && <>{t('reader-to', { who: m.recipients.join(', ') })} · </>}
            <span className="mono">{m.from_addr}</span>
          </span>
        </span>
        <Tip label={fullTime(m.date_ms)}>
          <time className="mono msg-time" dateTime={new Date(m.date_ms).toISOString()}>
            {messageTime(m.date_ms)}
          </time>
        </Tip>
      </header>

      <MessageBody messageId={m.id} title={m.subject || '(no subject)'} />

      {m.attachments.length > 0 && (
        <div className="msg-attachments">
          {m.attachments.map((a) => (
            <button type="button" className="att" key={a.filename}>
              <Icon icon={Paperclip} size={12} />
              {a.filename}
              <span className="mono att-size">{fileSize(a.size)}</span>
            </button>
          ))}
        </div>
      )}
    </article>
  );
}

export function Reader({
  thread,
  onAction,
  onMove,
  onTag,
  onSnooze,
}: {
  thread: Thread | null;
  onAction: (kind: ActionKind) => void;
  onMove: () => void;
  onTag: () => void;
  onSnooze: () => void;
}) {
  const [messages, setMessages] = useState<ThreadMessage[]>([]);
  const [expanded, setExpanded] = useState<Set<number>>(new Set());
  const [error, setError] = useState<string | null>(null);
  // Which message [ and ] move from. Separate from `expanded` because you can
  // have several open at once and still be reading one of them.
  const [focused, setFocused] = useState<number | null>(null);

  useEffect(() => {
    let live = true;
    setMessages([]);
    setExpanded(new Set());
    setError(null);
    if (!thread) return;
    api
      .threadDetail(thread.thread_id)
      .then((ms) => {
        if (!live) return;
        api.log(`thread_detail ok thread=${thread.thread_id} messages=${ms.length}`);
        setMessages(ms);
        // The newest message is what you came for; older ones stay folded until
        // asked for, so a five-message thread does not open as five walls of text.
        const last = ms[ms.length - 1];
        setExpanded(new Set(last ? [last.id] : []));
        setFocused(last?.id ?? null);
      })
      // Never swallow this: an empty reading pane and a failed call look
      // identical to the user, and only one of them is worth reporting.
      .catch((err: unknown) => {
        if (!live) return;
        setError(String(err));
        api.log(`thread_detail FAILED thread=${thread.thread_id}: ${err}`);
      });
    return () => {
      live = false;
    };
  }, [thread?.thread_id]);

  // [ and ] walk the conversation. Handled here rather than in the global map
  // for the same reason j/k live in the list: the keys mean "within the thing
  // in front of you", and the component holding that thing is the only one
  // that knows what is in it.
  //
  // Moving expands what it lands on. A navigation key that leaves you looking
  // at a collapsed line has not taken you anywhere.
  const messagesRef = useRef(messages);
  messagesRef.current = messages;
  const focusedRef = useRef(focused);
  focusedRef.current = focused;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      if (e.key !== '[' && e.key !== ']') return;
      const el = e.target instanceof HTMLElement ? e.target : null;
      if (
        el &&
        (el.tagName === 'INPUT' ||
          el.tagName === 'TEXTAREA' ||
          el.isContentEditable)
      ) {
        return;
      }
      if (document.querySelector('[role="dialog"]:not([hidden])')) return;

      const list = messagesRef.current;
      if (list.length === 0) return;
      e.preventDefault();

      const at = list.findIndex((m) => m.id === focusedRef.current);
      const nextIndex = at < 0 ? list.length - 1 : at + (e.key === ']' ? 1 : -1);
      const target = list[nextIndex];
      if (!target) return;

      setFocused(target.id);
      setExpanded((prev) => new Set(prev).add(target.id));
      // After the expand has rendered, or there is nothing to scroll to.
      requestAnimationFrame(() => {
        document
          .getElementById(`msg-body-${target.id}`)
          ?.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
      });
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  if (!thread) {
    return (
      <section className="reader" aria-label={t('reader-none-title')}>
        <div className="empty">
          <h2>{t('reader-none-title')}</h2>
          <p>{t('reader-none-body')}</p>
        </div>
      </section>
    );
  }

  const subject = thread.subject || '(no subject)';

  const hidden = messages.filter((m) => !expanded.has(m.id));
  const foldable = hidden.slice(0, Math.max(0, hidden.length - 1));
  const showAll = () => setExpanded(new Set(messages.map((m) => m.id)));

  return (
    <section className="reader" aria-label={subject}>
      <header className="reader-head">
        <div className="reader-headrow">
          <div className="reader-title">
            <h1 className="reader-subject">{subject}</h1>
            <div className="reader-meta">
              {thread.participants || thread.from_display || thread.from_addr}
              {thread.message_count > 1 && (
                <>
                  {' · '}
                  <span className="mono">
                    {t('reader-message-count', { count: fmtCount(thread.message_count) })}
                  </span>
                </>
              )}
            </div>
          </div>
          <div className="reader-actions">
            <Tip label={thread.starred ? t('menu-unstar') : t('menu-star')} keys={['S']}>
              <button
                type="button"
                className={`act-icon${thread.starred ? ' on' : ''}`}
                aria-label={t('reader-star')}
                aria-pressed={thread.starred}
                onClick={() => onAction(thread.starred ? 'unstar' : 'star')}
              >
                <Icon icon={Star} />
              </button>
            </Tip>
            <Tip label={t('reader-archive')} keys={['E']}>
              <button
                type="button"
                className="act-icon"
                aria-label={t('reader-archive')}
                onClick={() => onAction('archive')}
              >
                <Icon icon={Archive} />
              </button>
            </Tip>
            {/* A conversation you are reading is read by definition, so in
                practice this is the "put it back, I will deal with it later"
                gesture — which is why it belongs here and not only on the row. */}
            <Tip
              label={thread.unread ? t('reader-mark-read') : t('reader-mark-unread')}
              keys={[thread.unread ? key('read') : key('unread')]}
            >
              <button
                type="button"
                className="act-icon"
                aria-label={thread.unread ? t('reader-mark-read') : t('reader-mark-unread')}
                onClick={() => onAction(thread.unread ? 'mark_read' : 'mark_unread')}
              >
                <Icon icon={thread.unread ? MailOpen : Mail} />
              </button>
            </Tip>
            <MoreMenu
              thread={thread}
              onAction={onAction}
              onMove={onMove}
              onTag={onTag}
              onSnooze={onSnooze}
            />
          </div>
        </div>
      </header>

      <div className="reader-body">
        {error && (
          <div className="empty">
            <h2 style={{ color: 'var(--danger)' }}>{t('reader-failed')}</h2>
            <p className="mono" style={{ fontSize: 11.5 }}>{error}</p>
          </div>
        )}
        {messages.map((m) => {
          const isExpanded = expanded.has(m.id);
          // Collapse a run of older messages into one row rather than a stack of
          // near-identical lines.
          const foldStart = foldable.length > 1 && m.id === foldable[1]?.id;
          if (foldStart) {
            return (
              <button type="button" className="collapsed fold" key={`fold-${m.id}`} onClick={showAll}>
                <span className="mono collapsed-snip">
                  {t('reader-earlier', { count: foldable.length })}
                </span>
                <Icon icon={ChevronDown} size={14} />
              </button>
            );
          }
          if (!isExpanded && foldable.some((f, fi) => fi > 1 && f.id === m.id)) return null;
          return isExpanded ? (
            <Expanded
              m={m}
              key={m.id}
              focused={focused === m.id}
              onCollapse={() =>
                setExpanded((prev) => {
                  const next = new Set(prev);
                  next.delete(m.id);
                  return next;
                })
              }
            />
          ) : (
            <Collapsed m={m} key={m.id} onExpand={() => setExpanded((s) => new Set(s).add(m.id))} />
          );
        })}

        {messages.length > 0 && (
          <div className="reply-row">
            <button type="button" className="reply primary">
              <Icon icon={CornerUpLeft} size={14} />
              {t('reader-reply')} <span className="kbd on-accent">R</span>
            </button>
            <button type="button" className="reply">
              {t('reader-reply-all')} <span className="kbd">A</span>
            </button>
            <button type="button" className="reply">
              {t('reader-forward')} <span className="kbd">F</span>
            </button>
          </div>
        )}
      </div>
    </section>
  );
}
