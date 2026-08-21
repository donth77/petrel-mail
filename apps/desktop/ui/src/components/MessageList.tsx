import { useCallback, useEffect, useMemo, useRef } from 'react';
import { Composite, CompositeItem, useCompositeStore } from '@ariakit/react';
import { useVirtualizer, defaultRangeExtractor, type Range } from '@tanstack/react-virtual';
import { Archive, Clock, Paperclip, Star } from 'lucide-react';
import type { Thread } from '../lib/api';
import { Icon } from './Icon';
import { initials, listTime, fullTime } from '../lib/format';
import { t } from '../lib/strings';

type Props = {
  items: Thread[];
  activeId: number | null;
  density: 'relaxed' | 'compact';
  onActivate: (id: number) => void;
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

export function MessageList({ items, activeId, density, onActivate }: Props) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const composite = useCompositeStore({
    // DOM focus stays on the scroller and aria-activedescendant points at the row.
    // This is what makes virtualization survivable: a row unmounting as it
    // scrolls away cannot strand real focus, because focus was never on it.
    virtualFocus: true,
    orientation: 'vertical',
    focusLoop: false,
  });

  const rowHeight = density === 'compact' ? 44 : 68;
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
  });

  // Gmail's j/k, mapped onto the composite's own movement so both routes share
  // one notion of "where am I".
  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    if (e.key === 'j') {
      e.preventDefault();
      composite.move(composite.next());
    } else if (e.key === 'k') {
      e.preventDefault();
      composite.move(composite.previous());
    }
  };

  // Follow the active row when it moves outside the rendered window.
  useEffect(() => {
    if (activeIndex >= 0) virtualizer.scrollToIndex(activeIndex, { align: 'auto' });
  }, [activeIndex, virtualizer]);

  const virtualRows = virtualizer.getVirtualItems();

  return (
    <Composite
      store={composite}
      role="listbox"
      aria-label={t('a11y-message-list')}
      className={`scroller density-${density}`}
      ref={scrollRef}
      onKeyDown={onKeyDown}
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
              style={{ height: v.size, transform: `translateY(${v.start}px)` }}
              onClick={() => onActivate(m.id)}
              onFocus={() => onActivate(m.id)}
            >
              <span className="avatar" aria-hidden="true">
                {initials(m.from_display, m.from_addr)}
              </span>
              <span className="row-main">
                <span className="row-top">
                  {m.unread && <span className="unread-pip" aria-hidden="true" />}
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
                  {m.has_attachments && <Icon icon={Paperclip} size={12} className="ic-clip" />}
                  {m.subject || '(no subject)'}
                </span>
                {density === 'relaxed' && (
                  <span className="row-snippet clip">
                    <Snippet text={m.snippet} />
                  </span>
                )}
              </span>
              {/* A div, not buttons: this row is already a button, and nesting
                  interactive elements is invalid and unreachable by keyboard.
                  These are pointer affordances for actions the keyboard reaches
                  with E and B. */}
              <span className="row-actions" aria-hidden="true">
                <span className="quick" title="Archive (E)">
                  <Icon icon={Archive} size={14} />
                </span>
                <span className="quick" title="Snooze (B)">
                  <Icon icon={Clock} size={14} />
                </span>
              </span>
            </CompositeItem>
          );
        })}
      </div>
    </Composite>
  );
}
