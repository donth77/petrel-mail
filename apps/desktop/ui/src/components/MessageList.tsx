import { useCallback, useEffect, useMemo, useRef } from 'react';
import { Composite, CompositeItem, useCompositeStore } from '@ariakit/react';
import { useVirtualizer, defaultRangeExtractor, type Range } from '@tanstack/react-virtual';
import { Archive, Clock, MoreVertical, Paperclip, Star } from 'lucide-react';
import type { ActionKind, Thread } from '../lib/api';
import { Icon } from './Icon';
import { initials, listTime, fullTime } from '../lib/format';
import { t } from '../lib/strings';

type Props = {
  items: Thread[];
  activeId: number | null;
  density: 'relaxed' | 'compact';
  onActivate: (id: number) => void;
  onAction: (kind: ActionKind, threadId: number) => void;
  onMore: (threadId: number) => void;
  onNotImplemented: (label: string) => void;
};

/** Marks up `[bracketed]` spans from the engine's snippet as search hits. */
function Snippet({ text }: { text: string }) {
  const parts = text.split(/(\[[^\]]*\])/g);
  return (
    <>
      {parts.map((p, i) =>
        p.startsWith('[') && p.endsWith(']') ? <mark key={i}>{p.slice(1, -1)}</mark> : p,
      )}
    </>
  );
}

export function MessageList({
  items,
  activeId,
  density,
  onActivate,
  onAction,
  onMore,
  onNotImplemented,
}: Props) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const composite = useCompositeStore({
    // DOM focus stays on the scroller and aria-activedescendant points at the row.
    // This is what makes virtualization survivable: a row unmounting as it
    // scrolls away cannot strand real focus, because focus was never on it.
    virtualFocus: true,
    orientation: 'vertical',
    focusLoop: false,
  });

  // An estimate only — rows carry chips when they have attachments or tags, so
  // real heights vary and the virtualizer measures each mounted row.
  const rowHeight = density === 'compact' ? 30 : 74;
  const activeIndex = useMemo(
    () => items.findIndex((m) => m.id === activeId),
    [items, activeId],
  );

  // Keep the active row mounted even when scrolled out of view — otherwise
  // aria-activedescendant points at an element that no longer exists.
  const rangeExtractor = useCallback(
    (range: Range) => {
      const base = defaultRangeExtractor(range);
      if (activeIndex >= 0 && !base.includes(activeIndex)) {
        return [...base, activeIndex].sort((a, b) => a - b);
      }
      return base;
    },
    [activeIndex],
  );

  const virtualizer = useVirtualizer({
    count: items.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => rowHeight,
    overscan: 8,
    rangeExtractor,
    measureElement: (el) => el.getBoundingClientRect().height,
  });

  // Movement lives here rather than in Ariakit's composite navigation, because
  // the app needs to know the selection changed — moving the highlight without
  // changing what the reading pane shows is just watching the list scroll.
  //
  // Gmail's j/k are global, not scoped to whether the list happens to hold
  // focus. Scoping them to the scroller meant they did nothing until you had
  // clicked into the list first, which is not how anyone expects them to work.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      // The target is not always an element — a key pressed with nothing
      // focused reports the document or the window, neither of which has
      // closest(). Guarding on instanceof keeps this from throwing and
      // silently killing the handler.
      const el = e.target instanceof HTMLElement ? e.target : null;
      if (
        el &&
        (el.tagName === 'INPUT' ||
          el.tagName === 'TEXTAREA' ||
          el.tagName === 'SELECT' ||
          el.isContentEditable)
      ) {
        return;
      }
      // Inside a dialog the list is not what the keys are for — and testing the
      // event target is not enough, because with nothing focused the target is
      // the body, which has no dialog ancestor. Ask whether a dialog is *open*,
      // not where the keystroke came from: Ariakit keeps closed dialogs mounted
      // and `hidden`, so the selector has to exclude those or it always matches.
      if (document.querySelector('[role="dialog"]:not([hidden])')) return;

      const down = e.key === 'j' || e.key === 'ArrowDown';
      const up = e.key === 'k' || e.key === 'ArrowUp';
      // j/k are global; arrows only when the list holds focus, because
      // everywhere else an arrow key is expected to scroll.
      const scoped = e.key === 'ArrowDown' || e.key === 'ArrowUp';
      if (scoped && !scrollRef.current?.contains(document.activeElement)) return;

      if (down || up) {
        e.preventDefault();
        // Direction is computed from the list's own order, not delegated to
        // the composite's next()/previous(). Those depend on which items are
        // currently registered — and with virtualization that set changes as you
        // scroll — so the same key could resolve differently depending on where
        // you were. Index arithmetic over `items` is the visual order by
        // definition: j is always the row below, k always the row above.
        const focusedItem = el?.closest<HTMLElement>('[role="option"]');
        const fromId = focusedItem?.id?.startsWith('msg-')
          ? Number(focusedItem.id.slice(4))
          : activeId;
        const at = items.findIndex((m) => m.id === fromId);
        const nextIndex = at < 0 ? 0 : at + (down ? 1 : -1);
        const target = items[nextIndex];
        if (!target) return;

        scrollRef.current?.focus({ preventScroll: true });
        composite.setActiveId(`msg-${target.id}`);
        onActivate(target.id);
      }
    };
    // Bubble phase, deliberately. With virtualFocus Ariakit re-dispatches the
    // keydown onto the active item as a non-bubbling clone, so a capture-phase
    // listener sees the same keypress twice and the selection jumps two rows.
    // On bubble only the original arrives. Ariakit may still move its own
    // activeId first; harmless, because the line below *sets* the target rather
    // than stepping from wherever the composite happens to be.
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [composite, items, activeId, onActivate]);

  // Give the list focus once it has something to show, so arrow keys work
  // without a click first. Only when nothing else has been focused deliberately.
  const focused = useRef(false);
  useEffect(() => {
    if (focused.current || items.length === 0) return;
    const active = document.activeElement;
    if (!active || active === document.body) {
      scrollRef.current?.focus({ preventScroll: true });
      focused.current = true;
    }
  }, [items.length]);

  // `activeId` is the single source of truth for what is selected; the composite
  // only follows it. An earlier version also pushed the composite's id back into
  // the app, and the two effects ping-ponged — each one "correcting" the other
  // from a stale read until React gave up. Movement now has exactly one writer
  // on each side: keys and clicks set the app's activeId, this mirrors it down.
  useEffect(() => {
    const want = activeId == null ? null : `msg-${activeId}`;
    if (composite.getState().activeId !== want) composite.setActiveId(want);
  }, [activeId, composite]);

  // Follow the active row when the *selection* moves — not on every render.
  // `virtualizer` is a fresh object each render, so depending on it re-ran this
  // effect constantly and snapped the scroll position back to the active row,
  // which reads as "the list will not scroll".
  const lastScrolledTo = useRef(-1);
  useEffect(() => {
    if (activeIndex >= 0 && activeIndex !== lastScrolledTo.current) {
      lastScrolledTo.current = activeIndex;
      virtualizer.scrollToIndex(activeIndex, { align: 'auto' });
    }
  }, [activeIndex, virtualizer]);

  const virtualRows = virtualizer.getVirtualItems();

  return (
    <Composite
      store={composite}
      role="listbox"
      aria-label={t('a11y-message-list')}
      className={`scroller density-${density}`}
      ref={scrollRef}
    >
      <div className="rows" style={{ height: virtualizer.getTotalSize() }}>
        {virtualRows.map((v) => {
          const m = items[v.index];
          if (!m) return null;
          return (
            <CompositeItem
              key={m.id}
              store={composite}
              id={`msg-${m.id}`}
              role="option"
              aria-selected={m.id === activeId}
              // The whole point of these two: 30 rows are in the DOM out of
              // 100,000, so without them a screen reader says "3 of 30" while
              // the user is at message 4,187.
              aria-setsize={items.length}
              aria-posinset={v.index + 1}
              aria-label={t('a11y-row', {
                unread: m.unread ? t('a11y-unread-prefix') : '',
                from: m.from_display || m.from_addr,
                subject: m.subject || '(no subject)',
                time: fullTime(m.date_ms),
              })}
              className="row"
              data-active={m.id === activeId}
              data-unread={m.unread}
              data-index={v.index}
              ref={virtualizer.measureElement}
              style={{ transform: `translateY(${v.start}px)` }}
              onClick={() => {
                // Keep the composite's notion of "current" in step with the
                // click, so the keyboard carries on from where the pointer left.
                composite.setActiveId(`msg-${m.id}`);
                onActivate(m.id);
              }}
            >
              {/* The unread dot has its own grid column, so a read row does not
                  shift its avatar and text leftward to fill the gap. */}
              <span aria-hidden="true">
                {m.unread && <span className="unread-dot" />}
              </span>

              {density === 'compact' ? (
                // Compact is a different row, not relaxed with things hidden:
                // no avatar, one line, fixed-width sender and time so the
                // subjects align into a column the eye can run down.
                <span className="crow">
                  <span className="crow-from clip">
                    {m.message_count > 1 && m.participants
                      ? m.participants
                      : m.from_display || m.from_addr}
                  </span>
                  {/* Siblings, not inline children: an inline icon inside the
                      subject grows that span's line box and makes the row taller
                      than the density it is named for. */}
                  {m.starred && <Icon icon={Star} size={11} className="ic-star flat" />}
                  <span className="crow-subject clip">{m.subject || '(no subject)'}</span>
                  {m.attachment_name && <Icon icon={Paperclip} size={11} className="ic-clip" />}
                  {m.message_count > 1 && (
                    <span className="thread-count">{m.message_count}</span>
                  )}
                  <span className="crow-time">{listTime(m.date_ms)}</span>
                </span>
              ) : (
                <>
                  <span className="avatar" aria-hidden="true">
                    {initials(m.from_display, m.from_addr)}
                  </span>
                  <span className="row-main">
                    <span className="row-top">
                      {/* Participants once a conversation has more than one voice —
                          "who is in this" matters more than "who spoke last". */}
                      <span className="row-from clip">
                        {m.message_count > 1 && m.participants
                          ? m.participants
                          : m.from_display || m.from_addr}
                      </span>
                      {m.message_count > 1 && (
                        <span className="thread-count">{m.message_count}</span>
                      )}
                      <span className="row-time">{listTime(m.date_ms)}</span>
                    </span>
                    <span className="row-subject clip">
                      {m.starred && <Icon icon={Star} size={12} className="ic-star" />}
                      {m.subject || '(no subject)'}
                    </span>
                    <span className="row-snippet clip">
                      <Snippet text={m.snippet} />
                    </span>
                    {(m.attachment_name || m.tags.length > 0) && (
                      <span className="row-chips">
                        {m.attachment_name && (
                          <span className="rchip">
                            <Icon icon={Paperclip} size={10} />
                            <span className="clip">{m.attachment_name}</span>
                          </span>
                        )}
                        {m.tags.map((tag) => (
                          <span
                            key={tag.name}
                            className="rchip"
                            style={tag.colour ? { color: tag.colour, borderColor: tag.colour } : undefined}
                          >
                            {tag.name}
                          </span>
                        ))}
                      </span>
                    )}
                  </span>
                </>
              )}

              {/* Spans, not buttons: the row is already a button and nesting
                  interactive elements is invalid and unreachable by keyboard.
                  These are pointer affordances for what E, B and the palette
                  already do, so they are hidden from assistive tech. */}
              <span className="row-actions" aria-hidden="true">
                {/* stopPropagation, or the click also lands on the row behind
                    and selects the conversation we are about to archive. */}
                <span
                  className="qact"
                  title={t('qact-archive')}
                  onClick={(e) => {
                    e.stopPropagation();
                    onAction('archive', m.id);
                  }}
                >
                  <Icon icon={Archive} size={14} />
                </span>
                <span
                  className="qact"
                  title={t('qact-snooze')}
                  onClick={(e) => {
                    e.stopPropagation();
                    onNotImplemented(t('reader-snooze'));
                  }}
                >
                  <Icon icon={Clock} size={14} />
                </span>
                <span
                  className="qact"
                  title={t('qact-more')}
                  onClick={(e) => {
                    e.stopPropagation();
                    onMore(m.id);
                  }}
                >
                  <Icon icon={MoreVertical} size={14} />
                </span>
              </span>
            </CompositeItem>
          );
        })}
      </div>
    </Composite>
  );
}
