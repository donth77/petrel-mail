import { useEffect, useMemo, useRef, useState } from 'react';
import { api, type Account, type Folder, type Tag, type Thread, type Status } from './lib/api';
import { count as fmtCount } from './lib/format';
import { t, type StringId } from './lib/strings';
import { Search } from 'lucide-react';
import { Rail } from './components/Rail';
import { useKeyboard } from './lib/useKeyboard';
import { useTriage, type UndoOffer } from './lib/useTriage';
import { TitleBar } from './components/TitleBar';
import { Palette } from './components/Palette';
import { Picker, type PickerOption } from './components/Picker';
import { notifiable, postDesktopNotification } from './lib/notify';
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
  const [picker, setPicker] = useState<'folder' | 'tag' | null>(null);
  const [folders, setFolders] = useState<Folder[]>([]);

  useEffect(() => {
    let live = true;
    let handle: ReturnType<typeof setTimeout>;
    const tick = () =>
      api.status().then((s) => {
        if (!live) return;
        setStatus(s);
        // Keep asking after the first sync finishes, not just during it. The
        // engine polls the server every couple of minutes; if the window stops
        // listening once seeding ends, mail arrives into the store and nothing
        // on screen ever changes.
        handle = setTimeout(tick, s.seeding ? 400 : 5000);
      });
    tick();
    return () => {
      live = false;
      clearTimeout(handle);
    };
  }, []);

  // Debounced as-you-type search; an empty box falls back to the listing.
  useEffect(() => {
    let live = true;
    const run = () => {
      const p = query.trim() ? api.search(query) : api.threads(view, 0, 500);
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
  }, [query, view, status?.count, status?.seeding]);

  const triage = useTriage({
    items,
    setItems,
    activeId,
    setActiveId,
    view,
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
    openMove: () => setPicker('folder'),
    openTag: () => setPicker('tag'),
    openPalette: () => setPaletteOpen(true),
    openHelp: () => setHelpOpen(true),
    openSettings: () => setSettingsOpen(true),
    focusSearch: () => searchRef.current?.focus(),
  });

  // The rail key is the view's identity; its label comes from the same string
  // table the rail uses, so the two can never disagree.
  const viewName = useMemo(
    () => (view.startsWith('tag:') ? view.slice(4) : t(`mailbox-${view}` as StringId)),
    [view],
  );

  // An empty list means different things in different views, and saying the
  // wrong one is worse than saying nothing: "Nothing in Sent" reads as a fact
  // about your mail when it is really a fact about what Petrel cannot do yet.
  const emptyState = useMemo(() => {
    if (query) {
      return {
        title: t('empty-search-title', { query }),
        body: t('empty-search-body', { count: fmtCount(status?.count ?? 0) }),
      };
    }
    if (view === 'inbox') {
      return { title: t('empty-inbox-title'), body: t('empty-inbox-body') };
    }
    if (view === 'snoozed' || view === 'outbox') {
      return {
        title: t('empty-notbuilt-title', { view: viewName }),
        body: t('empty-notbuilt-body'),
      };
    }
    const body =
      // Starred is not somewhere you move mail to, so the generic copy is
      // wrong there in a way a reader would notice.
      view === 'starred' ? t('empty-starred-body', { key: 'S' })
      : view === 'sent' ? t('empty-sent-body')
      : view === 'drafts' ? t('empty-drafts-body')
      : t('empty-view-body');
    return { title: t('empty-view-title', { view: viewName }), body };
  }, [query, view, viewName, status?.count]);

  const active = useMemo(() => items.find((m) => m.id === activeId) ?? null, [items, activeId]);

  // Announce mail that arrived while the window was open.
  //
  // Keyed on ids rather than a count: a count that goes up and down as things
  // are archived would announce the same message twice, and comparing counts
  // cannot tell "two arrived" from "one arrived and one left".
  const announced = useRef<Set<number> | null>(null);
  useEffect(() => {
    if (view !== 'inbox' || query) return;
    // The first list is the mailbox as it already was, not an arrival. Seeding
    // it silently is what stops a first launch from announcing 200 messages.
    if (announced.current === null) {
      if (items.length > 0 || !status?.seeding) {
        announced.current = new Set(items.map((m) => m.id));
      }
      return;
    }
    const fresh = items.filter((m) => !announced.current!.has(m.id));
    items.forEach((m) => announced.current!.add(m.id));
    if (fresh.length === 0) return;

    const worth = notifiable(settings, fresh, Date.now());
    if (worth.length === 0) return;

    const top = worth[0];
    const who = top.from_display || top.from_addr;
    setToast(
      worth.length === 1
        ? t('notify-one', { who })
        : t('notify-many', { count: fmtCount(worth.length) }),
    );
    if (settings.notifyDesktop === 'on') {
      void postDesktopNotification(
        who,
        worth.length === 1 ? top.subject || '(no subject)' : t('notify-many', { count: fmtCount(worth.length) }),
      );
    }
  }, [items, view, query, settings, status?.seeding]);

  // Opening a conversation marks it read, as every mail client does.
  //
  // After a dwell, not instantly: j/k moves the selection one row at a time, so
  // marking on selection would clear the unread state of every conversation you
  // scrolled past on the way to the one you wanted. The delay is short enough to
  // feel automatic and long enough to survive a fast scroll.
  useEffect(() => {
    if (!active?.unread || settings.layout === 'off') return;
    const id = active.id;
    const h = setTimeout(() => void triage.run('mark_read', id, undefined, true), 900);
    return () => clearTimeout(h);
  }, [active?.id, active?.unread, settings.layout, triage]);

  const unread = useMemo(() => items.filter((m) => m.unread).length, [items]);

  // Tags come from the account, so one that has no conversation on this page
  // still appears in the rail.
  const [tags, setTags] = useState<Tag[]>([]);
  const [accounts, setAccounts] = useState<Account[]>([]);

  // Folders show their full path so "Contracts/2026" is distinguishable from
  // another "2026" elsewhere; tags carry their colour and whether this
  // conversation already has them, because tagging is a set, not a choice.
  const pickerOptions: PickerOption[] = useMemo(() => {
    if (picker === 'tag') {
      const on = new Set((active?.tags ?? []).map((x) => x.name));
      return tags.map((tg) => ({
        id: tg.id,
        label: tg.name,
        colour: tg.colour || undefined,
        on: on.has(tg.name),
      }));
    }
    return folders.map((f) => ({ id: f.id, label: f.path }));
  }, [picker, folders, tags, active]);
  useEffect(() => {
    let live = true;
    api.tags().then((t) => live && setTags(t)).catch(() => {});
    api.folders().then((f) => live && setFolders(f)).catch((e) => api.log(`folders failed: ${e}`));
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
      {status?.sync_error && (
        // Loud on purpose. A sync that fails silently is indistinguishable from
        // an account with no mail in it, and that ambiguity cost real time.
        <div className="sync-error" role="alert">
          <strong>{t('sync-failed-title')}</strong>
          <span>{status.sync_error}</span>
          <span className="sync-error-note">{t('sync-failed-body')}</span>
        </div>
      )}
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
            <span className="view-name">{viewName}</span>
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
        ) : loading || (status?.seeding && items.length === 0) ? (
          // A sync in flight with nothing ingested yet is not an empty mailbox,
          // and saying "Inbox is clear" while mail is arriving is the most
          // convincing possible way to report a working sync as a broken one.
          <div className="empty">
            <p>{status?.seeding ? t('empty-syncing', { count: fmtCount(status.count) }) : t('empty-loading')}</p>
          </div>
        ) : items.length === 0 ? (
          <div className="empty">
            <h2>{emptyState.title}</h2>
            <p>{emptyState.body}</p>
          </div>
        ) : (
          <MessageList
            items={items}
            activeId={activeId}
            density={settings.density}
            onActivate={setActiveId}
            onAction={(kind, threadId) => void triage.run(kind, threadId)}
            onNotImplemented={(label) => setToast(t('not-implemented', { label }))}
          />
        )}
      </div>

      {(settings.layout !== 'off' || readerOverlay) && (
        <Reader
          thread={active}
          onAction={(kind) => void triage.run(kind)}
          onMove={() => setPicker('folder')}
          onTag={() => setPicker('tag')}
        />
      )}

      <Picker
        open={picker !== null}
        mode={picker ?? 'folder'}
        subject={active?.subject ?? null}
        options={pickerOptions}
        onClose={() => setPicker(null)}
        onChoose={(id, on) => {
          if (picker === 'folder') {
            void triage.run('move', undefined, id);
            setPicker(null);
          } else {
            // Toggling: `on` is the state being moved to, so an applied tag
            // untags rather than re-applying and reporting "Tagged" twice.
            void triage.run(on ? 'tag' : 'untag', undefined, id);
          }
        }}
        onCreate={(name) => {
          const make = picker === 'folder' ? api.createFolder(name) : api.createTag(name);
          void make
            .then((id) => {
              if (picker === 'folder') {
                setPicker(null);
                return triage.run('move', undefined, id).then(() => api.folders().then(setFolders));
              }
              return triage.run('tag', undefined, id).then(() => api.tags().then(setTags));
            })
            .catch((e) => setToast(t('triage-failed', { error: String(e) })));
        }}
      />

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
