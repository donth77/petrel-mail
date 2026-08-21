import { useEffect, useMemo, useRef, useState } from 'react';
import { api, type Tag, type Thread, type Status } from './lib/api';
import { count as fmtCount } from './lib/format';
import { t } from './lib/strings';
import { Search } from 'lucide-react';
import { Rail } from './components/Rail';
import { TitleBar } from './components/TitleBar';
import { Palette } from './components/Palette';
import { Help } from './components/Help';
import { Toast } from './components/Toast';
import { MessageList } from './components/MessageList';
import { Reader } from './components/Reader';

export function App() {
  const [status, setStatus] = useState<Status | null>(null);
  const [items, setItems] = useState<Thread[]>([]);
  const [query, setQuery] = useState('');
  const [activeId, setActiveId] = useState<number | null>(null);
  const [view, setView] = useState('inbox');
  const [density] = useState<'relaxed' | 'compact'>('relaxed');
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [helpOpen, setHelpOpen] = useState(false);
  const [toast, setToast] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    const tick = () =>
      api.status().then((s) => {
        if (!live) return;
        setStatus(s);
        if (s.seeding) setTimeout(tick, 400);
      });
    tick();
    return () => {
      live = false;
    };
  }, []);

  // Debounced as-you-type search; an empty box falls back to the listing.
  useEffect(() => {
    let live = true;
    const run = () => {
      const p = query.trim() ? api.search(query) : api.threads(0, 500);
      p.then((rows: Thread[]) => {
        if (!live) return;
        setError(null);
        setItems(rows);
        setLoading(false);
        setActiveId((cur) => (rows.some((r: Thread) => r.id === cur) ? cur : (rows[0]?.id ?? null)));
      }).catch((err: unknown) => {
        if (!live) return;
        setLoading(false);
        setError(String(err));
        api.log(`list/search failed: ${err}`);
      });
    };
    const h = setTimeout(run, query ? 100 : 0);
    return () => {
      live = false;
      clearTimeout(h);
    };
  }, [query, status?.count, status?.seeding]);

  // Single-key shortcuts pause inside text fields (docs 06 §14).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const el = e.target as HTMLElement | null;
      const typing =
        el && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.isContentEditable);
      // ⌘K works everywhere, including inside a text field — it is how you get
      // out of one. Single-key shortcuts do not.
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') {
        e.preventDefault();
        setPaletteOpen(true);
      } else if (e.key === '?' && !typing) {
        e.preventDefault();
        setHelpOpen(true);
      } else if (e.key === '/' && !typing) {
        e.preventDefault();
        searchRef.current?.focus();
      } else if (e.key === 'Escape' && typing) {
        (el as HTMLInputElement).blur();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  useEffect(() => {
    if (loading || error) return;
    // Two frames: the virtualizer cannot report rows until it has measured the
    // scroll element, so probing synchronously always reads zero.
    const h = requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        const sc = document.querySelector('.scroller');
        const rendered = document.querySelectorAll('.row').length;
        const setsize = document.querySelector('.row')?.getAttribute('aria-setsize');
        api.log(
          `ui-ready items=${items.length} rendered=${rendered} ` +
            `setsize=${setsize ?? 'none'} scroller=${sc?.clientWidth ?? 0}x${sc?.clientHeight ?? 0}`,
        );
      }),
    );
    return () => cancelAnimationFrame(h);
  }, [loading, error, items.length]);

  const active = useMemo(() => items.find((m) => m.id === activeId) ?? null, [items, activeId]);
  const unread = useMemo(() => items.filter((m) => m.unread).length, [items]);

  // Tags come from the account, so one that has no conversation on this page
  // still appears in the rail.
  const [tags, setTags] = useState<Tag[]>([]);
  useEffect(() => {
    let live = true;
    api.tags().then((t) => live && setTags(t)).catch(() => {});
    return () => {
      live = false;
    };
  }, [status?.count, status?.seeding]);

  // status.source is a description, not always an address — do not slice a
  // sentence at "@" and present the fragment as an account name.
  const accountLabel = (status?.source ?? '').includes('@')
    ? status!.source.split('@')[0]
    : t('app-name');

  return (
    <div className="app-frame">
      <TitleBar synced={status?.seeding ? t('status-seeding') : t('titlebar-sync')} />
      <div className="shell">
      <Rail
        account={status?.source ?? t('app-name')}
        unread={unread}
        view={view}
        tags={tags}
        onView={(v) => (v === 'help' ? setHelpOpen(true) : setView(v))}
      />

      <div className="list-pane">
        <div className="list-head">
          <div className="search-box">
            <Search size={14} strokeWidth={1.8} aria-hidden="true" style={{ color: 'var(--ink3)', flexShrink: 0 }} />
            <input
              ref={searchRef}
              className="search"
              type="search"
              value={query}
              placeholder={t('search-placeholder')}
              onChange={(e) => setQuery(e.target.value)}
              aria-label={t('search-placeholder')}
            />
            <span className="kbd">{t('search-hint-key')}</span>
          </div>
          <div className="view-row">
            <span className="view-name">{t('mailbox-inbox')}</span>
            <span className="chip">
              <span className="dot" style={{ background: 'var(--accent)' }} />
              {accountLabel}
            </span>
            <span className="view-count">{t('list-unread', { count: fmtCount(unread) })}</span>
          </div>
        </div>

        {error ? (
          <div className="empty">
            <h2 style={{ color: 'var(--danger)' }}>Could not load this mailbox</h2>
            <p className="mono" style={{ fontSize: 11.5 }}>{error}</p>
          </div>
        ) : loading ? (
          <div className="empty">
            <p>{t('empty-loading')}</p>
          </div>
        ) : items.length === 0 ? (
          <div className="empty">
            <h2>
              {query
                ? t('empty-search-title', { query })
                : t('empty-inbox-title')}
            </h2>
            <p>
              {query
                ? t('empty-search-body', { count: fmtCount(status?.count ?? 0) })
                : t('empty-inbox-body')}
            </p>
          </div>
        ) : (
          <MessageList
            items={items}
            activeId={activeId}
            density={density}
            onActivate={setActiveId}
          />
        )}
      </div>

      <Reader thread={active} />

      <Palette
        open={paletteOpen}
        onClose={() => setPaletteOpen(false)}
        subject={active?.subject ?? null}
        ctx={{
          hasThread: !!active,
          onView: (v) => {
            if (v === 'help') setHelpOpen(true);
            else if (v === 'search') searchRef.current?.focus();
            else setView(v);
          },
          onNotImplemented: (label) => setToast(t('not-implemented', { label })),
        }}
      />
      <Help open={helpOpen} onClose={() => setHelpOpen(false)} />
      <Toast message={toast} onDone={() => setToast(null)} />

      <footer className="status">
        <span style={{ display: 'inline-flex', alignItems: 'center', gap: 6 }}>
          <span className="dot" style={{ background: 'var(--good)', inlineSize: 6, blockSize: 6 }} />
          {status?.seeding ? t('status-seeding') : t('status-synced')}
        </span>
        <span style={{ color: 'var(--hair)' }}>|</span>
        <span>
          {t('status-counts', { count: fmtCount(items.length), unread: fmtCount(unread) })}
        </span>
        <span className="spacer" />
        <span>
          <span className="kbd">J</span> <span className="kbd">K</span> move
        </span>
        <span>
          <span className="kbd">/</span> search
        </span>
        <span>
          <span className="kbd">⌘K</span> commands
        </span>
      </footer>
      </div>
    </div>
  );
}
