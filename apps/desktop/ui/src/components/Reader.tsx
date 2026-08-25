import { useEffect, useRef, useState } from 'react';
import { Menu, MenuButton, MenuItem, MenuProvider } from '@ariakit/react';
import {
  Archive,
  ChevronDown,
  CornerUpLeft,
  Mail,
  MailOpen,
  MoreVertical,
  Reply as ReplyIcon,
  Star,
} from 'lucide-react';
import { api, type ActionKind, type Thread, type ThreadMessage } from '../lib/api';
import { count as fmtCount, fullTime, initials, messageTime } from '../lib/format';
import { FindBar } from './FindBar';
import { Icon } from './Icon';
import { MessageBody } from './MessageBody';
import { Attachments } from './Attachments';
import { MoreMenu } from './MoreMenu';
import { Tip } from './Tip';
import { key } from '../lib/keys';
import { useHoveredLink } from '../lib/links';
import { t } from '../lib/strings';
import { Unsubscribe } from './Unsubscribe';

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
  onReply,
  onForward,
  onToast,
  onComposeMailto,
}: {
  m: ThreadMessage;
  focused: boolean;
  onCollapse: () => void;
  onReply?: (messageId: number, all: boolean) => void;
  onForward?: (messageId: number) => void;
  onToast: (text: string) => void;
  onComposeMailto?: (to: string, subject: string) => void;
}) {
  return (
    <article className="msg" id={`msg-body-${m.id}`} data-focused={focused || undefined}>
      {/* The toggle is the header's own area rather than the whole header,
          because the actions live in it now and a button cannot contain
          buttons — the markup is invalid and assistive technology reads the
          nested controls as part of the outer one's label.

          Clicking it still collapses, the way it does in every mail client: if
          clicking a collapsed message opens it, clicking the open one has to
          close it again, or the only way back is to leave the conversation. A
          real button, so Enter and Space work without being reimplemented and
          it announces itself as expanded. */}
      <header className="msg-head">
        <button
          type="button"
          className="msg-toggle"
          aria-expanded="true"
          aria-label={t('reader-collapse', { who: m.from_display || m.from_addr })}
          onClick={onCollapse}
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
        </button>
        <Tip label={fullTime(m.date_ms)}>
          <time className="mono msg-time" dateTime={new Date(m.date_ms).toISOString()}>
            {messageTime(m.date_ms)}
          </time>
        </Tip>
        {/* Answering *this* message, as against the row at the foot of the
            conversation, which answers the newest. A thread is a sequence of
            different questions from different people, and forwarding one
            message out of the middle of a long one had no other way to happen.

            Absent in the popped-out window, which has no composer to open. */}
        {(onReply || onForward) && (
          <div className="msg-acts">
            <Unsubscribe
              messageId={m.id}
              sender={m.from_display || m.from_addr}
              onToast={onToast}
              onComposeMailto={onComposeMailto}
            />
            {onReply && (
              <Tip label={t('msg-reply')}>
                <button
                  type="button"
                  className="act-icon"
                  aria-label={t('msg-reply')}
                  onClick={() => onReply(m.id, false)}
                >
                  <Icon icon={ReplyIcon} size={15} />
                </button>
              </Tip>
            )}
            <MenuProvider placement="bottom-end">
              <Tip label={t('msg-actions')}>
                <MenuButton className="act-icon" aria-label={t('msg-actions')}>
                  <Icon icon={MoreVertical} size={15} />
                </MenuButton>
              </Tip>
              <Menu portal gutter={6} className="menu" aria-label={t('msg-actions')}>
                {onReply && (
                  <MenuItem className="menu-item" onClick={() => onReply(m.id, true)}>
                    {t('msg-reply-all')}
                  </MenuItem>
                )}
                {onForward && (
                  <MenuItem className="menu-item" onClick={() => onForward(m.id)}>
                    {t('msg-forward')}
                  </MenuItem>
                )}
                <MenuItem
                  className="menu-item"
                  onClick={() =>
                    void api.printMessage(m.id).catch((e) => onToast(String(e)))
                  }
                >
                  {t('msg-print')}
                </MenuItem>
              </Menu>
            </MenuProvider>
          </div>
        )}
      </header>

      <MessageBody messageId={m.id} title={m.subject || '(no subject)'} />

      {m.attachments.length > 0 && (
        <Attachments messageId={m.id} attachments={m.attachments} onToast={onToast} />
      )}
    </article>
  );
}

export function Reader({
  thread,
  view,
  full,
  finding,
  onCloseFind,
  onToggleFull,
  onPopOut,
  onAction,
  onMove,
  onMoveInbox,
  onTag,
  onSnooze,
  onReplyTo,
  onForwardFrom,
  onToast,
  onComposeMailto,
}: {
  thread: Thread | null;
  /** Reply to one message of the thread rather than to its newest. Absent in
      the popped-out window, which has no composer to open. */
  onReplyTo?: (messageId: number, all: boolean) => void;
  onForwardFrom?: (messageId: number) => void;
  /** Where outcomes are reported — "Saved contract.pdf", or why not. */
  onToast: (text: string) => void;
  onComposeMailto?: (to: string, subject: string) => void;
  /** Which view is open, so the destructive action can mean the right thing. */
  view: string;
  /** Reading pane has the window to itself. */
  full: boolean;
  /** Find-in-conversation is up. Owned above, because ⌘F is a global key and
   *  the bar has to survive this pane re-rendering under it. */
  finding?: boolean;
  onCloseFind?: () => void;
  /** Both optional, and omitted rather than disabled where they mean nothing —
   *  a conversation already alone in its own window can be neither expanded
   *  nor popped out again, and a button that does nothing is worse than one
   *  that is not there. */
  onToggleFull?: () => void;
  onPopOut?: () => void;
  onAction: (kind: ActionKind) => void;
  onMove: () => void;
  onMoveInbox?: () => void;
  onTag: () => void;
  onSnooze: () => void;
}) {
  const [messages, setMessages] = useState<ThreadMessage[]>([]);
  const [expanded, setExpanded] = useState<Set<number>>(new Set());
  const [error, setError] = useState<string | null>(null);
  // Which message [ and ] move from. Separate from `expanded` because you can
  // have several open at once and still be reading one of them.
  const [focused, setFocused] = useState<number | null>(null);
  const bodyRef = useRef<HTMLDivElement>(null);

  // With the other hooks, above the empty-pane early return: a hook called
  // after it runs on some renders and not others, which is not a hook.
  const hoveredLink = useHoveredLink();

  // Scrolling a long message from the keyboard.
  //
  // Nothing else can do this. The body renders in a sandboxed frame that is
  // sized to its own content and told not to scroll, so there is no scrollable
  // region inside it; the scroll lives out here, on a div the frame cannot
  // reach. And once the frame has focus — which it does the moment anyone
  // clicks a message — every key goes to it, and only the *identity* of the key
  // comes back out. So the app has to do the scrolling on the frame\'s behalf.
  //
  // Listening on window rather than the element for exactly that reason: the
  // forwarded keys are re-dispatched there, and a listener on the div would
  // never see the ones that matter most.
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      const body = bodyRef.current;
      const reader = body?.closest('.reader');
      // Only when the reading pane is where the user is. Otherwise Space would
      // scroll a message while they were working down the list.
      if (!body || !reader || !reader.contains(document.activeElement)) return;
      // Not while a control has focus. Space on a focused button means press
      // it, and stealing that to scroll would break every action in the header
      // for anyone working without a mouse.
      const on = document.activeElement as HTMLElement | null;
      const tag = on?.tagName;
      if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'BUTTON' || tag === 'A') return;
      if (on?.getAttribute('role') === 'button' || on?.isContentEditable) return;

      const page = body.clientHeight * 0.9;
      const by =
        e.key === ' ' ? (e.shiftKey ? -page : page)
        : e.key === 'PageDown' ? page
        : e.key === 'PageUp' ? -page
        : e.key === 'ArrowDown' ? 60
        : e.key === 'ArrowUp' ? -60
        : null;

      if (by !== null) {
        e.preventDefault();
        body.scrollBy({ top: by, behavior: 'instant' as ScrollBehavior });
        return;
      }
      // Home and End are the "start again" and "how does it end" keys, and on
      // a long message both are otherwise a lot of scrolling.
      if (e.key === 'Home' || e.key === 'End') {
        e.preventDefault();
        body.scrollTo({ top: e.key === 'Home' ? 0 : body.scrollHeight, behavior: 'instant' as ScrollBehavior });
      }
    }
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);


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
  const newest = messages[messages.length - 1];

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
            {/* Reading room, then a room of its own. Two different needs:
                one long message wants the width this window can give it, and
                a message you are working *from* wants to stay open while you
                do something else in the app. */}
            <MoreMenu
              thread={thread}
              view={view}
              onPopOut={onPopOut}
              full={full}
              onToggleFull={onToggleFull}
              onAction={onAction}
              onMove={onMove}
              onMoveInbox={onMoveInbox}
              onTag={onTag}
              onSnooze={onSnooze}
            />
          </div>
        </div>
      </header>

      <div className="reader-body" ref={bodyRef} tabIndex={-1}>
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
              onReply={onReplyTo}
              onForward={onForwardFrom}
              onToast={onToast}
              onComposeMailto={onComposeMailto}
              
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

        {/* These answer the newest message, which is what the conversation's
            Reply means everywhere — the per-message controls in each header
            answer the one they sit on. Both routes go through the same call so
            the button and the R key cannot come to mean different things.

            Shown only where there is something to open. The popped-out window
            has no composer, and three buttons that do nothing when pressed are
            worse than three buttons that are not there. */}
        {messages.length > 0 && newest && (onReplyTo || onForwardFrom) && (
          <div className="reply-row">
            {onReplyTo && (
              <button
                type="button"
                className="reply primary"
                onClick={() => onReplyTo(newest.id, false)}
              >
                <Icon icon={CornerUpLeft} size={14} />
                {t('reader-reply')} <span className="kbd on-accent">R</span>
              </button>
            )}
            {onReplyTo && (
              <button type="button" className="reply" onClick={() => onReplyTo(newest.id, true)}>
                {t('reader-reply-all')} <span className="kbd">A</span>
              </button>
            )}
            {onForwardFrom && (
              <button type="button" className="reply" onClick={() => onForwardFrom(newest.id)}>
                {t('reader-forward')} <span className="kbd">F</span>
              </button>
            )}
          </div>
        )}
      </div>

      {/* Where a browser puts it: bottom-left, over the content rather than
          displacing it, so the page does not shift every time the pointer
          crosses a link. Truncated by CSS rather than here — the beginning of
          a URL is the part that says who you are about to visit, and cutting
          it from the front to fit is how a deceptive link stays deceptive. */}
      {hoveredLink && !finding && (
        <div className="link-peek mono" role="status">
          {hoveredLink}
        </div>
      )}

      {/* At the foot of the pane, as every find bar is — out of the way of what
          you are reading, and where the eye is not. */}
      {onCloseFind && <FindBar open={Boolean(finding)} onClose={onCloseFind} />}
    </section>
  );
}
