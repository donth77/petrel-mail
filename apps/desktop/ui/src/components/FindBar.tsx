import { useCallback, useEffect, useRef, useState } from 'react';
import { ChevronDown, ChevronUp, X } from 'lucide-react';
import { Icon } from './Icon';
import { t } from '../lib/strings';

/**
 * Find in this conversation.
 *
 * The searching happens inside the message frames, because nothing out here can
 * read them: each body is opaque-origin by design, so the host cannot walk its
 * text and `window.find` would search the app's own chrome instead. Each frame
 * is told the term and reports how many it found.
 *
 * Stepping between matches is this component's job rather than any one frame's,
 * because a conversation is several frames and "next" has to cross from the end
 * of one message into the beginning of the next. It keeps the per-frame counts
 * in document order and turns one global position into a frame and an offset
 * within it.
 */
export function FindBar({ open, onClose }: { open: boolean; onClose: () => void }) {
  const [query, setQuery] = useState('');
  // Counts per frame, in document order. A plain array indexed the same way as
  // the frames themselves, so the arithmetic below stays obvious.
  const [counts, setCounts] = useState<number[]>([]);
  const [at, setAt] = useState(0);
  const field = useRef<HTMLInputElement>(null);

  const frames = useCallback(
    () => [...document.querySelectorAll<HTMLIFrameElement>('.reader .msg-frame')],
    [],
  );

  const total = counts.reduce((sum, n) => sum + n, 0);

  useEffect(() => {
    if (open) field.current?.focus();
  }, [open]);

  // Collect what each frame reports. Identified by its window rather than by an
  // id we invent: the frames are anonymous to us, and `source` is the only
  // handle a message from one carries.
  useEffect(() => {
    function onMessage(e: MessageEvent) {
      const n = (e.data as { petrelFound?: unknown })?.petrelFound;
      if (typeof n !== 'number') return;
      const i = frames().findIndex((f) => f.contentWindow === e.source);
      if (i < 0) return;
      setCounts((cur) => {
        const next = [...cur];
        while (next.length <= i) next.push(0);
        next[i] = n;
        return next;
      });
    }
    window.addEventListener('message', onMessage);
    return () => window.removeEventListener('message', onMessage);
  }, [frames]);

  // Send the term to every frame. Also on close, with an empty term, so the
  // highlights do not outlive the bar that made them.
  useEffect(() => {
    const term = open ? query : '';
    setCounts([]);
    setAt(0);
    for (const f of frames()) f.contentWindow?.postMessage({ petrelFind: term }, '*');
  }, [query, open, frames]);

  // Point the right frame at the right match. Walking the counts is what turns
  // "match 7 of 12" into "the second one in the third message".
  useEffect(() => {
    if (total === 0) return;
    let remaining = at;
    const list = frames();
    for (let i = 0; i < list.length; i += 1) {
      const n = counts[i] ?? 0;
      if (remaining < n) {
        list[i].contentWindow?.postMessage({ petrelFindActive: remaining }, '*');
        return;
      }
      remaining -= n;
    }
  }, [at, counts, total, frames]);

  if (!open) return null;

  const step = (by: number) => {
    if (total === 0) return;
    setAt((cur) => (cur + by + total) % total);
  };

  return (
    <div className="findbar" role="search">
      <input
        ref={field}
        className="find-field"
        value={query}
        placeholder={t('find-placeholder')}
        aria-label={t('find-placeholder')}
        autoComplete="off"
        spellCheck={false}
        onChange={(e) => setQuery(e.target.value)}
        onKeyDown={(e) => {
          // Held off the app's single-key shortcuts, like every other field.
          e.stopPropagation();
          if (e.key === 'Escape') onClose();
          // Enter walks forward, shift-Enter back — the convention every find
          // bar uses, and the reason this one needs no buttons to be usable.
          if (e.key === 'Enter') {
            e.preventDefault();
            step(e.shiftKey ? -1 : 1);
          }
        }}
      />

      <span className="find-count mono">
        {query.trim()
          ? total > 0
            ? t('find-position', { at: String(at + 1), total: String(total) })
            : t('find-none')
          : ''}
      </span>

      <button
        type="button"
        className="act-icon"
        aria-label={t('find-previous')}
        disabled={total === 0}
        onClick={() => step(-1)}
      >
        <Icon icon={ChevronUp} size={15} />
      </button>
      <button
        type="button"
        className="act-icon"
        aria-label={t('find-next')}
        disabled={total === 0}
        onClick={() => step(1)}
      >
        <Icon icon={ChevronDown} size={15} />
      </button>
      <button type="button" className="close-btn" aria-label={t('close')} onClick={onClose}>
        <Icon icon={X} size={15} />
      </button>
    </div>
  );
}
