import { useEffect, useMemo, useRef, useState } from 'react';
import { api, type Account, type Tag, type Thread, type Status } from './lib/api';
import { count as fmtCount } from './lib/format';
import { t } from './lib/strings';
import { Search } from 'lucide-react';
import { Rail } from './components/Rail';
import { useKeyboard } from './lib/useKeyboard';
import { useTriage, type UndoOffer } from './lib/useTriage';
import { TitleBar } from './components/TitleBar';
import { Palette } from './components/Palette';
import { Help } from './components/Help';
import { Settings } from './components/Settings';
import { useSettings } from './lib/settings';
import { Toast } from './components/Toast';
import { MessageList } from './components/MessageList';
import { Reader } from './components/Reader';

export function App() {
  const { settings } = useSettings();
  const [status, setStatus] = useState<Status | null>(null);
  const [items, setItems] = useState<Thread[]>([]);
  const [query, setQuery] = useState('');
  const [activeId, setActiveId] = useState<number | null>(null);
  const [view, setView] = useState('inbox');

  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [helpOpen, setHelpOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [toast, setToast] = useState<string | null>(null);
  const [undoOffer, setUndoOffer] = useState<UndoOffer | null>(null);
  const [readerOverlay, setReaderOverlay] = useState(false);

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

  const triage = useTriage({
    items,
    setItems,
    activeId,
    setActiveId,
    onMessage: (text, undo) => {
      setToast(text);
      setUndoOffer(undo ?? null);
    },
  });

  const railRef = useRef<HTMLElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  useKeyboard({
    openConversation: () => {
      // Enter opens what the list has focused; with the reading pane off it is
      // the only way to see a message at all.
      if (settings.layout === 'off') setReaderOverlay(true);
      else document.querySelector<HTMLElement>('.reader')?.focus();
    },
    backToList: () => {
      setReaderOverlay(false);
      document.querySelector<HTMLElement>('.scroller')?.focus();
    },
    cyclePanes: (backwards) => {
      // F6 is the platform convention for moving between panes, and the only
      // route into the rail without a pointer.
      const panes = [railRef.current, listRef.current?.querySelector('.scroller'), document.querySelector('.reader')]
        .filter(Boolean) as HTMLElement[];
      if (panes.length === 0) return;
      const at = panes.findIndex((p) => p.contains(document.activeElement));
      const next = (at + (backwards ? -1 : 1) + panes.length) % panes.length;
      const target = panes[next];
      target.setAttribute('tabindex', '-1');
      target.focus();
    },
    goTo: setView,
    triage: (kind) => void triage.run(kind),
    toggleStar: () => triage.toggleStar(),
    undo: () => void triage.undo(),
    switchAccount: (n) => {
      const acc = accounts[n - 1];
      if (acc) setToast(t('account-switched', { email: acc.email }));
      else setToast(t('account-none-at', { n: String(n) }));
    },
    openPalette: () => setPaletteOpen(true),
    openHelp: () => setHelpOpen(true),
    openSettings: () => setSettingsOpen(true),
    focusSearch: () => searchRef.current?.focus(),
  });

  const active = useMemo(() => items.find((m) => m.id === activeId) ?? null, [items, activeId]);
  const unread = useMemo(() => items.filter((m) => m.unread).length, [items]);

  // Tags come from the account, so one that has no conversation on this page
  // still appears in the rail.
  const [tags, setTags] = useState<Tag[]>([]);
  const [accounts, setAccounts] = useState<Account[]>([]);
  useEffect(() => {
    let live = true;
    api.tags().then((t) => live && setTags(t)).catch(() => {});
    api.accounts().then((a) => live && setAccounts(a)).catch(() => {});
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
      <div className="shell" data-layout={settings.layout === 'off' ? 'no-reader' : settings.layout}>
      <Rail
        account={accounts[0]?.email ?? status?.source ?? t('app-name')}
        accountColor={accounts[0]?.color || 'var(--accent)'}
        unread={unread}
        view={view}
        tags={tags}
        railRef={railRef}
        onView={(v) => {
          if (v === 'help') setHelpOpen(true);
          else if (v === 'settings') setSettingsOpen(true);
          else setView(v);
        }}
      />

      <div className="list-pane" ref={listRef}>
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
            density={settings.density}
            onActivate={setActiveId}
            onAction={(kind, threadId) => void triage.run(kind, threadId)}
            onMore={(threadId) => {
              // Same surface the reader's More opens — the palette already
              // scopes its commands to whichever conversation is selected.
              setActiveId(threadId);
              setPaletteOpen(true);
            }}
            onNotImplemented={(label) => setToast(t('not-implemented', { label }))}
          />
        )}
      </div>

      {(settings.layout !== 'off' || readerOverlay) && (
        <Reader
          thread={active}
          onAction={(kind) => void triage.run(kind)}
          onMore={() => setPaletteOpen(true)}
        />
      )}

      <Palette
        open={paletteOpen}
        onClose={() => setPaletteOpen(false)}
        subject={active?.subject ?? null}
        ctx={{
          hasThread: !!active,
          onView: (v) => {
            if (v === 'help') setHelpOpen(true);
            else if (v === 'settings') setSettingsOpen(true);
            else if (v === 'search') searchRef.current?.focus();
            else setView(v);
          },
          onNotImplemented: (label) => setToast(t('not-implemented', { label })),
        }}
      />
      <Help open={helpOpen} onClose={() => setHelpOpen(false)} />
      <Settings
        open={settingsOpen}
        onClose={() => {
          setSettingsOpen(false);
          api.accounts().then(setAccounts).catch(() => {});
        }}
        onOpenHelp={() => setHelpOpen(true)}
        onNotImplemented={(label) => setToast(t('not-implemented', { label }))}
      />
      <Toast
        message={toast}
        onUndo={
          undoOffer
            ? () => {
                void triage.undo(undoOffer);
                setUndoOffer(null);
              }
            : undefined
        }
        onDone={() => {
          setToast(null);
          setUndoOffer(null);
        }}
      />

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
