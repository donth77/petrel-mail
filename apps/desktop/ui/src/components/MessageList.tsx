import { useCallback, useEffect, useMemo, useRef } from 'react';
import { Composite, CompositeItem, useCompositeStore } from '@ariakit/react';
import { useVirtualizer, defaultRangeExtractor, type Range } from '@tanstack/react-virtual';
import { Archive, Check, Clock, Mail, MailOpen, Paperclip, Star } from 'lucide-react';
import type { ActionKind, Thread } from '../lib/api';
import { Icon } from './Icon';
import { initials, listTime, fullTime } from '../lib/format';
import { t } from '../lib/strings';
import { DRAG_TYPE, draggedIds } from '../lib/dnd';
import { Tip } from './Tip';
import { key } from '../lib/keys';

type Props = {
  items: Thread[];
  activeId: number | null;
  selected: ReadonlySet<number>;
  onToggleSelect: (id: number) => void;
  density: 'relaxed' | 'compact';
  /** Modifiers come along: cmd/ctrl toggles one, shift reaches back to the
   *  anchor, and a plain click means "just this one". Decided above, because
   *  the anchor and the selection live there. */
  onActivate: (id: number, mods: { toggle: boolean; range: boolean }) => void;
  /** A checkbox column down the left, from settings. */
  checkboxes: boolean;
  onAction: (kind: ActionKind, threadId: number) => void;
  onSnooze: (threadId: number) => void;
  /** Right-click. The pointer position comes along because a context menu is
   *  anchored to where you clicked, not to the row. */
  onContextMenu: (threadId: number, x: number, y: number) => void;
  /** What is being dragged, so the rail can show what will take it. */
  onDragIds: (ids: number[]) => void;
};

/** Marks up the engine's match markers as search hits.
 *
 * U+E000 and U+E001, not square brackets. Brackets are ordinary text in mail —
 * the plain-text alternative marketing senders generate is full of things like
 * [image: Google] — so with brackets as the marker this highlighted the
 * sender's own punctuation as though it had matched the search. A private-use
 * codepoint cannot be typed, pasted from a real message, or mistaken for
 * content. */
const MARK = /(\u{E000}[^\u{E001}]*\u{E001})/gu;

function Snippet({ text }: { text: string }) {
  return (
    <>
      {text.split(MARK).map((p, i) =>
        p.startsWith('\u{E000}') ? <mark key={i}>{p.slice(1, -1)}</mark> : p,
      )}
    </>
  );
}

export function MessageList({
  items,
  activeId,
  selected,
  onToggleSelect,
  density,
  onActivate,
  checkboxes,
  onAction,
  onSnooze,
  onContextMenu,
  onDragIds,
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
        // Keyboard movement is a plain move: J and K walk the list, and the
        // selection keys (X, ⇧J/K) are what act on more than one.
        onActivate(target.id, { toggle: false, range: false });
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

  // Follow the active row when the *selection* moves — not on every render, and
  // not when the list changes underneath a selection that has not.
  //
  // Keyed on which conversation is active rather than on where it sits. An
  // index is a position in a list, and replacing the list moves every position:
  // typing in the search box left the same conversation selected at a different
  // index, this read that as the selection moving, and threw the list to
  // wherever the row had landed — usually the end of a short result set.
  //
  // `virtualizer` is a fresh object each render, so it must not decide whether
  // to run; when it did, the effect fired constantly and snapped the scroll
  // back to the active row, which reads as "the list will not scroll".
  const lastScrolledFor = useRef<number | null>(null);
  useEffect(() => {
    if (activeId == null || activeIndex < 0) return;
    if (activeId === lastScrolledFor.current) return;
    lastScrolledFor.current = activeId;
    virtualizer.scrollToIndex(activeIndex, { align: 'auto' });
  }, [activeId, activeIndex, virtualizer]);

  const virtualRows = virtualizer.getVirtualItems();

  return (
    <Composite
      store={composite}
      role="listbox"
      aria-label={t('a11y-message-list')}
      className={`scroller density-${density}`}
      ref={scrollRef}
    >
      <div
        className="rows"
        // The row is a grid with a fixed column template, so the extra cell has
        // to be declared in CSS as well as rendered — inserting an element
        // without it pushes every later child one column along and the content
        // wraps into a second row.
        data-checkboxes={checkboxes || undefined}
        style={{ height: virtualizer.getTotalSize() }}
      >
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
              data-selected={selected.has(m.id) || undefined}
              data-unread={m.unread}
              data-index={v.index}
              ref={virtualizer.measureElement}
              style={{ transform: `translateY(${v.start}px)` }}
              onClick={(e: React.MouseEvent) => {
                // Keep the composite's notion of "current" in step with the
                // click, so the keyboard carries on from where the pointer left.
                composite.setActiveId(`msg-${m.id}`);
                onActivate(m.id, {
                  toggle: e.metaKey || e.ctrlKey,
                  range: e.shiftKey,
                });
              }}
              onContextMenu={(e) => {
                e.preventDefault();
                composite.setActiveId(`msg-${m.id}`);
                onContextMenu(m.id, e.clientX, e.clientY);
              }}
              draggable
              onDragStart={(e) => {
                const ids = draggedIds(m.id, selected);
                e.dataTransfer.setData(DRAG_TYPE, JSON.stringify(ids));
                e.dataTransfer.effectAllowed = 'move';
                onDragIds(ids);
                // Several rows cannot all be the drag image, and the one under
                // the pointer would misrepresent how many are moving. A count
                // says what is actually being carried.
                if (ids.length > 1) {
                  const ghost = document.createElement('div');
                  ghost.className = 'drag-ghost';
                  ghost.textContent = t('drag-count', { count: String(ids.length) });
                  document.body.appendChild(ghost);
                  e.dataTransfer.setDragImage(ghost, 12, 12);
                  // Removed on the next frame: it must survive being captured
                  // as the drag image, and must not linger in the document.
                  requestAnimationFrame(() => ghost.remove());
                }
              }}
              onDragEnd={() => onDragIds([])}
            >
              {/* The unread dot has its own grid column, so a read row does not
                  shift its avatar and text leftward to fill the gap. With the
                  checkbox column on it leaves the flow entirely — see the CSS. */}
              <span className="row-dot" aria-hidden="true">
                {m.unread && <span className="unread-dot" />}
              </span>

              {/* Compact has no avatar to click, so with the column off there
                  is nothing here to select with — which is exactly why the
                  checkbox has to render in both densities. A setting that
                  silently does nothing in one of them is worse than no
                  setting. */}
              {density === 'compact' && checkboxes && (
                <span
                  role="checkbox"
                  tabIndex={-1}
                  aria-checked={selected.has(m.id)}
                  aria-label={t('list-select-row')}
                  className="row-check"
                  onClick={(e) => {
                    e.stopPropagation();
                    onToggleSelect(m.id);
                  }}
                >
                  {selected.has(m.id) && <Icon icon={Check} size={11} />}
                </span>
              )}
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
                  {/* With the column on, the checkbox is the selection target
                      and the avatar goes back to being an avatar. Two targets
                      for one job is worse than either alone. */}
                  {checkboxes && (
                    <span
                      role="checkbox"
                      tabIndex={-1}
                      aria-checked={selected.has(m.id)}
                      aria-label={t('list-select-row')}
                      className="row-check"
                      onClick={(e) => {
                        e.stopPropagation();
                        onToggleSelect(m.id);
                      }}
                    >
                      {selected.has(m.id) && <Icon icon={Check} size={11} />}
                    </span>
                  )}
                  <span
                    className={checkboxes ? 'avatar' : 'avatar selectable'}
                    aria-hidden="true"
                    onClick={
                      checkboxes
                        ? undefined
                        : (e) => {
                            e.stopPropagation();
                            onToggleSelect(m.id);
                          }
                    }
                  >
                    {!checkboxes && selected.has(m.id) ? (
                      <Icon icon={Check} size={14} />
                    ) : (
                      initials(m.from_display, m.from_addr)
                    )}
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
                      {/* Why it matched, when it came from a search. The
                          ordinary opening line is the same on every row and
                          cannot say what the result was answering. */}
                      <Snippet text={m.match_snippet ?? m.snippet} />
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
                  already do, so they are hidden from assistive tech.

                  Three, not four. The bar overlays the row's trailing edge, and
                  a fourth icon covered 37% of a 430px row — burying the
                  timestamp. "More" was the one to drop: its only job was to
                  open the palette, which already has its own shortcut and is a
                  first-class surface in this app. The reader header, which has
                  room, keeps it. */}
              <span className="row-actions" aria-hidden="true">
                {/* stopPropagation, or the click also lands on the row behind
                    and selects the conversation we are about to archive. */}
                <Tip label={t('qact-archive')} keys={['E']}>
                  <span
                    className="qact"
                    onClick={(e) => {
                      e.stopPropagation();
                      onAction('archive', m.id);
                    }}
                  >
                    <Icon icon={Archive} size={14} />
                  </span>
                </Tip>
                {/* Second, not last: this is a triage verb, not an overflow
                    item. The icon names the *action* — an open envelope on an
                    unread row means "mark this read" — which is how Gmail and
                    Outlook read, and it stays in the same place whichever
                    direction you are going. A clickable unread dot cannot do
                    that: on a read row there is no dot to click, so the one
                    direction that matters — flagging something to come back
                    to — would have no target at all. */}
                <Tip
                  label={m.unread ? t('qact-mark-read') : t('qact-mark-unread')}
                  keys={[m.unread ? key('read') : key('unread')]}
                >
                  <span
                    className="qact"
                    onClick={(e) => {
                      e.stopPropagation();
                      onAction(m.unread ? 'mark_read' : 'mark_unread', m.id);
                    }}
                  >
                    <Icon icon={m.unread ? MailOpen : Mail} size={14} />
                  </span>
                </Tip>
                <Tip label={t('qact-snooze')} keys={['B']}>
                  <span
                    className="qact"
                    onClick={(e) => {
                      e.stopPropagation();
                      onSnooze(m.id);
                    }}
                  >
                    <Icon icon={Clock} size={14} />
                  </span>
                </Tip>
              </span>
            </CompositeItem>
          );
        })}
      </div>
    </Composite>
  );
}
