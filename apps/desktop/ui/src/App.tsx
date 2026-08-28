import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  api,
  type Folder,
  type OutboxRow,
  type Status,
  type Thread,
} from './lib/api';
import { chips, hasToken, scopeFor, toggleToken, folderLeaf } from './lib/search-chips';
import { count as fmtCount, fileSize } from './lib/format';
import { t, type StringId } from './lib/strings';
import { Search } from 'lucide-react';
import { Rail } from './components/Rail';
import { useKeyboard } from './lib/useKeyboard';
import { useAppMenu } from './lib/menu';
import { useTriage, type UndoOffer } from './lib/useTriage';
import { TitleBar } from './components/TitleBar';
import { Palette } from './components/Palette';
import { Picker, type PickerOption } from './components/Picker';
import { Compose, addresses, type Draft } from './components/Compose';
import { snoozeOptions } from './lib/snooze';
import { promisesMissingAttachment } from './lib/compose-checks';
import { replyTargets } from './lib/reply';
import { forwardBody, replyBody } from './lib/quote';
import { dropMeaning } from './lib/dnd';
import { nestableRolePath, underAnchor } from './lib/folders';
import { useDrag } from './lib/useDrag';
import { useReferenceData } from './lib/useReferenceData';
import { useDropGuard } from './lib/useFileDrop';
import { AppDialogs } from './components/AppDialogs';
import { DragPreview } from './components/DragPreview';
import { startingBody, startingHtml } from './lib/signature';
import { ATTACHMENT_LIMIT, pickAttachments, stageDropped } from './lib/attachments';
import { extend, prune, targets, toggle } from './lib/selection';
import { notifiable, postDesktopNotification, shouldNotify } from './lib/notify';
import { Help } from './components/Help';
import { Settings } from './components/Settings';
import { RAIL_COLLAPSED, clampList, clampRail, useSettings } from './lib/settings';
import { useMessageLinks, type HomographRisk } from './lib/links';
import { RowMenu } from './components/RowMenu';
import { Toast } from './components/Toast';
import { MessageList } from './components/MessageList';
import { Reader } from './components/Reader';
import { Outbox } from './components/Outbox';
import { Onboarding } from './components/Onboarding';
import { syncState } from './lib/sync-status';
import { Dialog } from '@ariakit/react';
import { PaneResize } from './components/PaneResize';

export function App() {
  const { settings, locale, set } = useSettings();
  const [status, setStatus] = useState<Status | null>(null);
  // Bumped when the active account changes. Every effect that reads the
  // store keys on it, so a switch reloads the list, the counts, the tags, the
  // folders and the identity together — rather than each noticing separately
  // or, worse, some not noticing at all.
  const [accountEpoch, setAccountEpoch] = useState(0);
  const [items, setItems] = useState<Thread[]>([]);
  const [query, setQuery] = useState('');
  // Best match or newest, for a search. Not a saved preference: it answers a
  // different question about one search — "find the thing" against "retrace the
  // timeline" — and carrying last week's answer into today's search is wrong
  // more often than it is right.
  const [newestFirst, setNewestFirst] = useState(false);
  // Whether the search field has the user's attention, which is when the
  // filters are worth showing.
  const [searching, setSearching] = useState(false);
  // Find-in-conversation. Held here because ⌘F is a global key and the bar has
  // to survive the reading pane re-rendering under it.
  const [finding, setFinding] = useState(false);

  const [activeId, setActiveId] = useState<number | null>(null);
  const [view, setView] = useState('inbox');

  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [helpOpen, setHelpOpen] = useState(false);
  // Null when closed; otherwise the pane to open on.
  const [settingsOpen, setSettingsOpen] = useState<'accounts' | 'appearance' | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  const [undoOffer, setUndoOffer] = useState<UndoOffer | null>(null);
  const [readerOverlay, setReaderOverlay] = useState(false);
  const [picker, setPicker] = useState<'folder' | 'tag' | 'snooze' | 'send-later' | null>(null);
  const [selected, setSelected] = useState<Set<number>>(new Set());
  // Where a range grows from, so reversing direction shrinks it again.
  const [anchor, setAnchor] = useState<number | null>(null);
  const [draft, setDraft] = useState<Draft | null>(null);
  // Reset per composer, so each message gets its own single warning.
  const attachmentWarned = useRef(false);
  // The picker awaits, so the draft may have moved on by the time it returns.
  const draftRef = useRef<Draft | null>(null);
  draftRef.current = draft;
  // A message waiting out its undo window. It is held here, in the window, and
  // has not touched the network — which is the whole reason undo can cancel it
  // rather than chase it.
  // The send waiting out its undo window. The message itself is in the
  // outbox; this is only what the toast needs to count down and to name it.
  const [outgoing, setOutgoing] = useState<{ id: number; subject: string; left: number } | null>(null);
  const outgoingRef = useRef(outgoing);
  outgoingRef.current = outgoing;
  // Reference data — tags, folders, accounts, identity — one hook, one
  // effect. Called this early because the triage hook below reads tags to
  // show one on a row the moment it is applied.
  const { tags, setTags, folders, setFolders, accounts, setAccounts, activeAccount, identity } =
    useReferenceData(status?.seeding, accountEpoch);

  // The number on the Dock icon: unread in the inbox, added up across
  // accounts. Not the current view's unread, which is what the rail and the
  // footer show — that number is right for them because it names what you are
  // looking at, and wrong for a Dock badge, which would then change every time
  // you clicked a folder.
  //
  // "Mailbox counts: None" turns it off, on the grounds that someone who does
  // not want counts beside their mailboxes does not want one on the Dock.
  useEffect(() => {
    const total =
      settings.badges === 'off'
        ? null
        : accounts.reduce((sum, a) => sum + (a.unread_count ?? 0), 0);
    void api.setDockBadge(total).catch(() => {});
  }, [accounts, settings.badges]);
  // The view's true size; items.length is only the loaded window.
  const [viewTotal, setViewTotal] = useState<number | null>(null);

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
      const p = query.trim() ? api.search(query, newestFirst) : api.threads(view, 0, 500);
      p.then((rows: Thread[]) => {
        if (!live) return;
        setError(null);
        setItems(rows);
        setLoading(false);
        setActiveId((cur) => (rows.some((r: Thread) => r.id === cur) ? cur : (rows[0]?.id ?? null)));
        // A selection pointing at rows that have gone would make the next
        // action target nothing and look broken.
        setSelected((cur) => (cur.size === 0 ? cur : prune(cur, rows.map((r: Thread) => r.id))));
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
  }, [query, view, newestFirst, status?.count, status?.seeding, accountEpoch]);

  // A new query starts at the top.
  //
  // Without this the browser decides: the old scroll position usually exceeds
  // the height of a shorter result list, so it is clamped to whatever the new
  // bottom happens to be — and typing into the search box appeared to throw the
  // list to the end. Results are ranked, so the top is also where the answer is.
  useEffect(() => {
    const scroller = listRef.current?.querySelector<HTMLElement>('.scroller');
    if (scroller) scroller.scrollTop = 0;
  }, [query, view, newestFirst]);

  // The tag awaiting confirmation. Deleting one takes it off every
  // conversation carrying it, which is not a thing to do on one click.
  const [deletingTag, setDeletingTag] = useState<{ id: number; name: string } | null>(null);
  const [deletingFolder, setDeletingFolder] = useState<Folder | null>(null);
  const [movingFolder, setMovingFolder] = useState<Folder | null>(null);
  // The outbox message awaiting a discard confirmation. Discarding is the one
  // outbox action with no undo: the message was never sent, so there is
  // nothing to recall it from.
  const [discarding, setDiscarding] = useState<OutboxRow | null>(null);


  // Resolving a tag id to what a row displays. The rail's list is already the
  // authority on names and colours, so the patch reads from it rather than
  // inventing a second copy that could disagree.
  const tagById = useCallback(
    (id: number) => {
      const tag = tags.find((x) => x.id === id);
      return tag ? { name: tag.name, colour: tag.colour } : undefined;
    },
    [tags],
  );

  const triage = useTriage({
    tagById,
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

  // Which conversations a confirmed delete would remove. Captured when the
  // dialog opens rather than read when it closes: the selection can change
  // underneath an open dialog, and deleting something other than what the
  // dialog named is the worst possible outcome for the one action with no undo.
  // The reading pane with the window to itself. Not a saved preference: it is
  // a thing you do to one long message, not a way you like the app arranged —
  // and coming back tomorrow to a hidden list would read as a broken window.
  const [readerFull, setReaderFull] = useState(false);

  // Where a right-click landed, and on what. Held here rather than in the list
  // because acting on it needs the same triage, pickers and confirmation the
  // rest of the app uses.
  const [rowMenu, setRowMenu] = useState<{ id: number; x: number; y: number } | null>(null);

  useEffect(() => {
    // The matches belonged to the conversation that just closed.
    setFinding(false);
  }, [activeId]);

  const [pendingDelete, setPendingDelete] = useState<number[] | null>(null);
  // A draft opened while a foreign revision of it stands on the server.
  const [draftConflict, setDraftConflict] = useState<{ draftId: number; otherId: number } | null>(
    null,
  );
  // A link waiting on "is this really where you meant to go?".
  const [riskyLink, setRiskyLink] = useState<{ risk: HomographRisk; open: () => void } | null>(
    null,
  );
  // The Trash, waiting on "yes, permanently".
  const [emptyingTrash, setEmptyingTrash] = useState(false);
  const askDelete = (ids?: number[]) => {
    const list = ids ?? targets(selected, activeId);
    if (list.length > 0) setPendingDelete(list);
  };

  /** Opening a mailbox, which ends any search that was running.
   *
   * Search is not scoped to a view — the "In Inbox" chip is how you narrow it —
   * so leaving the query in place while switching left the list showing search
   * results under a header that named the mailbox. It said "Sent · 37 found"
   * over the same inbox-wide results, which is the header lying about what is
   * on screen. Picking a mailbox means you want that mailbox.
   */
  const goToView = (v: string) => {
    setQuery('');
    setView(v);
  };

  /** A blank message, signed. The C key, the palette and the File menu all
   *  come here — it was written out three times before the menu made a fourth
   *  copy unthinkable, and one of the three had already drifted: the palette
   *  forgot to clear the attachment warning, so a composer opened from there
   *  had used up its one warning before it began. */
  const startCompose = () => {
    attachmentWarned.current = false;
    setDraft({
      to: '',
      cc: '',
      subject: '',
      body: startingBody(identity, false),
      html: startingHtml(identity, false),
    });
  };

  const railRef = useRef<HTMLElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  /**
   * Pulls a send back out of its undo window and into the composer.
   *
   * Out of the outbox and back, text and all: the store has the whole
   * message, so nothing is reconstructed here. One function, because the
   * Z key and the bar's Undo button must mean the same thing.
   */
  const cancelPendingSend = () => {
    const o = outgoingRef.current;
    if (!o) return;
    setOutgoing(null);
    void api
      .outboxEdit(o.id)
      .then(() => resumeDraft(o.id))
      .then(() => setToast(t('compose-cancelled')))
      .catch((e) => setToast(t('compose-resume-failed', { error: String(e) })));
  };

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
    goTo: goToView,
    triage: (kind) => {
      // One key, more things. Acting on the selection when there is one is
      // what makes X worth having: nothing new to learn, it just applies to
      // more than one conversation.
      const ids = targets(selected, activeId);
      // Inside the trash there is nowhere further to move something, so the
      // bin key means the permanent thing — behind the dialog, never straight
      // through. Same key, same place, and the only irreversible one asks.
      if (kind === 'trash' && view === 'trash') {
        askDelete(ids);
        return;
      }
      ids.forEach((id) => void triage.run(kind, id));
      if (selected.size > 0) setSelected(new Set());
    },
    toggleStar: () => triage.toggleStar(),
    // Only where there is a reading pane to fill. With the layout off there is
    // no pane, and with nothing open there would be nothing to look at.
    findInMessage: () => {
      // Only where there is something to find in. With no reading pane, or
      // nothing open, ⌘F would put up a bar that could never match anything.
      if (settings.layout !== 'off' && activeRef.current) setFinding(true);
    },
    toggleReaderFull: () => {
      if (settings.layout !== 'off' && activeRef.current) setReaderFull((f) => !f);
    },
    undo: () => {
      // A pending send outranks the last triage action: it is the thing with a
      // deadline, and it is what the countdown just told you Z would do.
      if (outgoing) {
        cancelPendingSend();
        return;
      }
      void triage.undo();
    },
    switchAccount: (n) => {
      const acc = accounts[n - 1];
      if (!acc) {
        setToast(t('account-none-at', { n: String(n) }));
        return;
      }
      if (acc.active) return;
      void switchAccount(acc.id, acc.email);
    },
    compose: startCompose,
    reply: (all) => {
      // R follows the configured default; A always means reply-all.
      if (active) void startReply(active.id, all || settings.replyDefault === 'reply-all');
    },
    forward: () => {
      if (active) void startForward(active.id);
    },
    snooze: () => setPicker('snooze'),
    select: () => {
      if (activeId == null) return;
      setSelected((cur) => toggle(cur, activeId));
      setAnchor(activeId);
    },
    extendSelection: (down) => {
      const at = items.findIndex((m) => m.id === activeId);
      const next = items[at + (down ? 1 : -1)];
      if (!next) return;
      setSelected((cur) => extend(cur, items.map((m) => m.id), anchor ?? activeId, next.id));
      setActiveId(next.id);
    },
    clearSelection: () => {
      // Escape backs out of the most recent thing first. Filling the window is
      // more recent than a selection made before it, and leaving someone in a
      // list-less window because Escape went to the selection instead is the
      // kind of dead end that sends people back to the mouse.
      if (readerFull) {
        setReaderFull(false);
        return;
      }
      setSelected(new Set());
      setAnchor(null);
    },
    openMove: () => setPicker('folder'),
    openTag: () => setPicker('tag'),
    openPalette: () => setPaletteOpen(true),
    openHelp: () => setHelpOpen(true),
    openSettings: () => setSettingsOpen('appearance'),
    focusSearch: () => searchRef.current?.focus(),
  });

  // The macOS menu bar, driving the same functions as everything above it. Its
  // ⌘, arrives here rather than in useKeyboard now — the OS gives a menu's key
  // equivalents first refusal — which is only safe because both open the same
  // pane.
  useAppMenu({
    newMessage: startCompose,
    openSettings: () => setSettingsOpen('appearance'),
    openHelp: () => setHelpOpen(true),
    theme: settings.theme,
    density: settings.density,
    setTheme: (v) => set('theme', v),
    setDensity: (v) => set('density', v),
  });

  // The rail key is the view's identity; its label comes from the same string
  // table the rail uses, so the two can never disagree.
  const viewName = useMemo(
    () =>
      view.startsWith('tag:')
        ? view.slice(4)
        : view.startsWith('folder:')
          ? (() => {
              const f = folders.find((x) => `folder:${x.id}` === view);
              return f ? f.path.split(/[/.]/).pop() || f.path : t('rail-folders');
            })()
          : t(`mailbox-${view}` as StringId),
    // locale: the labels come from t(), which a re-render alone does not
    // refresh inside a memo.
    [view, folders, locale],
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

    const body =
      // Starred is not somewhere you move mail to, so the generic copy is
      // wrong there in a way a reader would notice.
      view === 'outbox' ? t('empty-outbox-body')
      : view === 'snoozed' ? t('empty-snoozed-body', { key: 'B' })
      : view === 'starred' ? t('empty-starred-body', { key: 'S' })
      : view === 'sent' ? t('empty-sent-body')
      : view === 'drafts' ? t('empty-drafts-body')
      : t('empty-view-body');
    return { title: t('empty-view-title', { view: viewName }), body };
  }, [query, view, viewName, status?.count, locale]);

  const active = useMemo(() => items.find((m) => m.id === activeId) ?? null, [items, activeId]);

  /**
   * Opens a reply to the newest message in a conversation.
   *
   * Recipients come from the message rather than the list row: the row only
   * knows who spoke last, so a reply-all built from it would leave off
   * everyone else who was on the thread.
   */
  /**
   * Starts a reply.
   *
   * `targetId` names the message being replied to. Without one it is the newest,
   * which is what the conversation-level Reply means — but a thread is a
   * sequence of different questions from different people, and answering the
   * third one should quote the third one and address whoever sent it.
   */
  /**
   * Conversations dropped on a rail destination.
   *
   * Routed through the same `triage.run` the keys and menus use, so a drag can
   * never come to mean something slightly different from the shortcut for the
   * same thing — and so undo, the toast and the optimistic list update all come
   * along without being reimplemented for the pointer.
   */
  const dropOnRail = (railKey: string, ids: number[]) => {
    const meaning = dropMeaning(railKey);
    if (!meaning || ids.length === 0) return;

    if (meaning.kind === 'tag') {
      const tag = tags.find((x) => x.name === meaning.tag);
      if (!tag) return;
      ids.forEach((id) => void triage.run('tag', id, tag.id));
    } else if (meaning.kind === 'move') {
      const folder = folders.find((f) => f.role === meaning.role);
      if (!folder) return;
      ids.forEach((id) => void triage.run('move', id, folder.id));
    } else if (meaning.kind === 'move-folder') {
      ids.forEach((id) => void triage.run('move', id, meaning.folderId));
    } else if (meaning.kind === 'trash' && view === 'trash') {
      // Trash is where deletion becomes permanent, and permanent is asked
      // about rather than dragged into.
      askDelete(ids);
      return;
    } else {
      ids.forEach((id) => void triage.run(meaning.kind, id));
    }
    if (selected.size > 0) setSelected(new Set());
  };

  const startReply = async (id: number, all: boolean, targetId?: number) => {
    const row = items.find((m) => m.id === id);
    if (!row) return;
    try {
      const messages = await api.threadDetail(row.thread_id);
      const last =
        (targetId != null ? messages.find((m) => m.id === targetId) : undefined) ??
        messages[messages.length - 1];
      if (!last) return;
      const { to, cc } = replyTargets(last, identity?.address ?? '', all);
      attachmentWarned.current = false;
      // The original, quoted. Fetched rather than taken from the row, which
      // carries a 120-character snippet — a reply quoting a preview would be
      // worse than one quoting nothing.
      const quoted = await api.quoteMessage(last.id).catch(() => null);
      setDraft({
        to: to.join(', '),
        cc: cc.join(', '),
        // Threading rides on the headers, not the subject; the Re: is only
        // what people expect to read.
        subject: row.subject.match(/^re:/i) ? row.subject : `Re: ${row.subject}`,
        body: startingBody(identity, true),
        html: quoted
          ? replyBody(
              startingHtml(identity, true),
              quoted.from,
              quoted.date_ms,
              quoted.html,
              settings.language === 'system' ? undefined : settings.language,
            )
          : startingHtml(identity, true),
        inReplyTo: null,
        references: [],
      });
    } catch (e) {
      setToast(t('compose-resume-failed', { error: String(e) }));
    }
  };

  /**
   * Starts a forward, carrying the message with it.
   *
   * `targetId` names which message of the thread to send on; without one it is
   * the newest. Forwarding used to open an empty draft with a `Fwd:` subject
   * and nothing under it, which is not a forward — the recipient got a subject
   * line and no message.
   */
  const startForward = async (id: number, targetId?: number) => {
    const row = items.find((m) => m.id === id);
    if (!row) return;
    try {
      const messages = await api.threadDetail(row.thread_id);
      const target =
        (targetId != null ? messages.find((m) => m.id === targetId) : undefined) ??
        messages[messages.length - 1];
      if (!target) return;
      attachmentWarned.current = false;
      const quoted = await api.quoteMessage(target.id).catch(() => null);
      const subject = quoted?.subject?.trim() || row.subject;
      setDraft({
        to: '',
        cc: '',
        subject: subject.match(/^fwd:/i) ? subject : `Fwd: ${subject}`,
        body: startingBody(identity, true),
        html: quoted
          ? forwardBody(
              startingHtml(identity, true),
              quoted.from,
              quoted.to,
              subject,
              quoted.date_ms,
              quoted.html,
              settings.language === 'system' ? undefined : settings.language,
            )
          : startingHtml(identity, true),
      });
    } catch (e) {
      setToast(t('compose-resume-failed', { error: String(e) }));
    }
  };

  /** Reopens a saved draft in the composer. */
  const resumeDraft = async (id: number) => {
    try {
      const d = await api.loadDraft(id);
      attachmentWarned.current = false;
      setDraft({
        to: d.to,
        cc: '',
        subject: d.subject,
        body: d.body,
        html: d.html,
        savedId: d.id,
      });
      // Asked as the draft opens, not at save time: the person is about to
      // continue from one of two versions, and should choose before typing
      // into the wrong one. The data layer kept both; that is what makes
      // this a question instead of a loss.
      const conflict = await api.draftConflict(id);
      if (conflict) setDraftConflict({ draftId: id, otherId: conflict.other_id });
    } catch (e) {
      setToast(t('compose-resume-failed', { error: String(e) }));
    }
  };

  /** Settles a draft conflict the chosen way and shows it in the composer. */
  const settleDraftConflict = async (takeServer: boolean) => {
    const c = draftConflict;
    setDraftConflict(null);
    if (!c) return;
    try {
      await api.resolveDraftConflict(c.draftId, c.otherId, takeServer);
      if (takeServer) {
        // Reopened rather than merged into place: the composer's editor
        // mounts its words once, and only a fresh mount shows the adopted
        // revision rather than the one already on screen.
        setDraft(null);
        await resumeDraft(c.draftId);
      }
      setToast(takeServer ? t('draft-took-server') : t('draft-kept-local'));
    } catch (e) {
      setToast(t('draft-conflict-failed', { error: String(e) }));
    }
  };

  /**
   * Shows a different account.
   *
   * The active account lives in the store, so one call changes what every
   * command reads; the epoch then makes the window re-read all of it. The
   * open conversation and any selection belong to the old account and are
   * dropped — a row id from one account means nothing in another.
   */
  const switchAccount = async (id: number, email: string) => {
    try {
      await api.setActiveAccount(id);
      setActiveId(null);
      setSelected(new Set());
      setView('inbox');
      setQuery('');
      setAccountEpoch((n) => n + 1);
      setToast(t('account-switched', { email }));
    } catch (e) {
      setToast(t('account-switch-failed', { error: String(e) }));
    }
  };

  /** The parts of a draft that are not its text, as the store keeps them. */
  const envelopeOf = (d: Draft) => ({
    cc: d.cc,
    inReplyTo: d.inReplyTo ?? null,
    references: d.references ?? [],
    attachments: (d.attachments ?? []).map((a) => a.path),
  });

  /**
   * Saves the composer's contents, and remembers the id so saving again
   * updates the same draft rather than leaving a trail of near-identical ones.
   */
  const saveDraft = async (d: Draft) => {
    try {
      const id = await api.saveDraft(d.savedId ?? null, d.to, d.subject, d.body, d.html, envelopeOf(d));
      setDraft((cur) => (cur ? { ...cur, savedId: id } : cur));
      setToast(t('compose-saved'));
      // The Drafts view is a query, so it only changes when the list reloads.
      if (view === 'drafts') api.threads(view, 0, 500).then(setItems).catch(() => {});
      return id;
    } catch (e) {
      setToast(t('compose-save-failed', { error: String(e) }));
      return null;
    }
  };

  /**
   * Attaches files the user picks.
   *
   * The size check happens here rather than at send, which is what the design
   * asks for and the only version that helps: refusing at send means the
   * message is written, the recipient chosen, and the failure arrives at the
   * moment there is nothing to do about it.
   */
  const attach = async () => {
    try {
      const current = draftRef.current;
      if (!current) return;
      const result = await pickAttachments(current.attachments ?? [], api.attachmentInfo);
      if (!result) return;
      setDraft({ ...current, attachments: result.kept });
      if (result.rejected.length > 0) {
        setToast(
          t('compose-too-large', {
            name: result.rejected.join(', '),
            limit: fileSize(ATTACHMENT_LIMIT),
          }),
        );
      }
    } catch (e) {
      setToast(t('compose-attach-failed', { error: String(e) }));
    }
  };

  // A file dropped anywhere but the composer would otherwise replace the
  // whole application with that file.
  useDropGuard();

  const { drag, start: startDrag, startTag, startFolder } = useDrag(
    view,
    dropOnRail,
    // A tag dropped onto a conversation. The same call the picker makes, so a
    // drag cannot come to mean something slightly different from the menu.
    (tagId, threadId) => void triage.run('tag', threadId, tagId),
    // A folder dropped onto a new parent: re-nesting is a rename, which on
    // IMAP is the move, and the store cascades it through the subtree.
    (folderId, targetPath) => {
      const f = folders.find((x) => x.id === folderId);
      if (!f) return;
      const leaf = f.path.split(/[/.]/).pop() ?? f.path;
      // Dropping a folder on Trash re-nests it under the trash folder — the
      // Thunderbird semantics: trash is a holding pen, and dragging back out
      // is the restore. Deletion proper stays in the menu, behind its
      // confirm, because a mis-aimed drag must never be the gesture that
      // destroys mail on the server.
      if (targetPath === '::trash') {
        const trash = nestableRolePath(folders, 'trash');
        if (!trash) return;
        void api
          .renameFolder(folderId, `${trash}/${leaf}`)
          .then(() => api.folders().then(setFolders))
          .then(() => setToast(t('folder-trashed', { name: leaf })))
          .catch((e) => setToast(t('folder-failed', { error: String(e) })));
        return;
      }
      const next = targetPath ? `${targetPath}/${leaf}` : leaf;
      if (next === f.path) return; // already there
      // A folder cannot become its own descendant — the rename would eat
      // its target mid-cascade and the tree would swallow itself.
      if (targetPath === f.path || targetPath.startsWith(`${f.path}/`)) {
        setToast(t('folder-into-itself'));
        return;
      }
      void api
        .renameFolder(folderId, next)
        .then(() => api.folders().then(setFolders))
        .then(() => setToast(t('folder-moved', { name: leaf, to: targetPath || t('rail-folders') })))
        .catch((e) => setToast(t('folder-failed', { error: String(e) })));
    },
    // Dropped in the gap between two rows: a reorder, not a move.
    //
    // The order is taken from the rendered rows rather than from the folders
    // array, because those are not the same sequence. Folders draw as a tree —
    // children nested under parents — so the visible order is a depth-first
    // walk, while the array is whatever the engine's sort returned. Splicing
    // the array moved folders to places nobody had pointed at.
    //
    // Reading the rows means the order saved is literally the order on screen,
    // and there is no second implementation of the tree to keep in step.
    (payload, at) => {
      // Conversations are never reordered, so a threads payload has no
      // business here and the narrowing says so rather than assuming.
      if (payload.kind !== 'folder' && payload.kind !== 'tag') return;
      const moving = payload.kind === 'folder' ? payload.folderId : payload.tagId;
      const rows = [...document.querySelectorAll<HTMLElement>('.rail [data-reorder]')];
      const ids = rows.map((r) => Number(r.dataset.reorder)).filter(Number.isFinite);

      // Folders and tags share the attribute but are separate lists, and one
      // must never be renumbered by a drag in the other.
      const known = new Set(
        payload.kind === 'folder' ? folders.map((f) => f.id) : tags.map((x) => x.id),
      );
      const list = ids.filter((id) => known.has(id));
      const from = list.indexOf(moving);
      if (from < 0) return;

      const next = list.slice();
      next.splice(from, 1);
      // Found again after the removal: taking it out shifts everything below.
      const target = next.indexOf(Number(at.key));
      if (target < 0) return;
      next.splice(at.edge === 'before' ? target : target + 1, 0, moving);
      if (next.join() === list.join()) return;

      if (payload.kind === 'folder') {
        const byId = new Map(folders.map((f) => [f.id, f]));
        setFolders(next.map((id) => byId.get(id)!).filter(Boolean));
        void api
          .reorderFolders(next)
          .catch((e) => setToast(t('folder-failed', { error: String(e) })));
      } else {
        const byId = new Map(tags.map((x) => [x.id, x]));
        setTags(next.map((id) => byId.get(id)!).filter(Boolean));
        void api
          .reorderTags(next)
          .catch((e) => setToast(t('tag-rename-failed', { error: String(e) })));
      }
    }
  );

  /** Files dragged onto the composer from the desktop. */
  const dropAttachments = async (files: FileList) => {
    try {
      const current = draftRef.current;
      if (!current) return;
      const result = await stageDropped(
        [...files],
        current.attachments ?? [],
        api.stageAttachment,
      );
      setDraft({ ...draftRef.current!, attachments: result.kept });
      if (result.rejected.length > 0) {
        setToast(
          t('compose-too-large', {
            name: result.rejected.join(', '),
            limit: fileSize(ATTACHMENT_LIMIT),
          }),
        );
      }
    } catch (e) {
      setToast(t('compose-attach-failed', { error: String(e) }));
    }
  };

  // The undo-send countdown. A clock on the toast, nothing more: the message
  // is in the outbox with its time set, and the outbox clock sends it. When
  // this reaches zero the toast goes away and the send is the outbox's affair
  // — which is exactly the point, because a toast that also sends is a send
  // that is lost the moment the window closes.
  useEffect(() => {
    if (!outgoing) return;
    if (outgoing.left <= 0) {
      setOutgoing(null);
      setToast(t('compose-sent'));
      return;
    }
    const h = setTimeout(() => setOutgoing((o) => (o ? { ...o, left: o.left - 1 } : null)), 1000);
    return () => clearTimeout(h);
  }, [outgoing]);

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

  // What a rule asked to announce. These never reach the inbox list the
  // effect above watches — the rule filed them — so their word rides the
  // status poll, already drained server-side so each is said once. The
  // pause and off switches still govern: a rule outranks the level (the
  // rule is a priority declaration), never the silence.
  useEffect(() => {
    const fresh = status?.notify ?? [];
    if (fresh.length === 0) return;
    if (!shouldNotify(settings, Date.now())) return;
    const [who, subject] = fresh[0];
    setToast(
      fresh.length === 1
        ? t('notify-one', { who })
        : t('notify-many', { count: fmtCount(fresh.length) }),
    );
    if (settings.notifyDesktop === 'on') {
      void postDesktopNotification(
        who,
        fresh.length === 1
          ? subject || '(no subject)'
          : t('notify-many', { count: fmtCount(fresh.length) }),
      );
    }
  }, [status, settings]);

  // Moving off a conversation marks it read — the rule every mail client with
  // a reading pane uses, and the one Outlook states outright as "mark as read
  // when the selection changes".
  //
  // An earlier version only marked the *current* conversation, after a dwell,
  // so that scrolling with j/k did not clear everything you passed. That was
  // wrong in the ordinary case: select one, click the next, and the first was
  // never marked at all, because moving cancelled the timer that would have
  // done it. Reading something and moving on is the common gesture; passing
  // over something on the way to somewhere else is the rare one.
  //
  // The dwell survives for whatever is selected *now*, so a conversation you
  // stop on is marked read without having to leave it.
  const autoRead = useRef<number | null>(null);
  const previousId = useRef<number | null>(null);
  const activeRef = useRef(active);
  activeRef.current = active;
  const itemsRef = useRef(items);
  itemsRef.current = items;
  const triageRef = useRef(triage);
  triageRef.current = triage;

  useEffect(() => {
    if (settings.layout === 'off') return;
    const current = activeRef.current;
    const leaving = previousId.current;
    previousId.current = current?.id ?? null;

    // The one you just left.
    if (leaving != null && leaving !== current?.id) {
      const row = itemsRef.current.find((m) => m.id === leaving);
      if (row?.unread && !triageRef.current.isHeldUnread(leaving)) {
        autoRead.current = leaving;
        void triageRef.current.run('mark_read', leaving, undefined, true);
      }
    }

    // Arriving at a conversation that was being held unread ends the hold:
    // you asked to come back to it, and this is coming back. Without this a
    // conversation marked unread once could never be marked read by reading
    // it again — and testing the rule by marking something unread first would
    // look exactly like the rule being broken.
    if (current && leaving !== current.id) {
      triageRef.current.releaseHeldUnread(current.id);
    }

    // And the one you are on, if you stay.
    if (!current || autoRead.current === current.id) return;
    if (!current.unread) {
      autoRead.current = current.id;
      return;
    }
    const id = current.id;
    const h = setTimeout(() => {
      if (triageRef.current.isHeldUnread(id)) return;
      autoRead.current = id;
      void triageRef.current.run('mark_read', id, undefined, true);
    }, 900);
    return () => clearTimeout(h);
  }, [active?.id, settings.layout]);

  const unread = useMemo(() => items.filter((m) => m.unread).length, [items]);

  const [counts, setCounts] = useState<Record<string, number>>({});

  // Folders show their full path so "Contracts/2026" is distinguishable from
  // another "2026" elsewhere; tags carry their colour and whether this
  // conversation already has them, because tagging is a set, not a choice.
  // Re-read the tags as the picker opens.
  //
  // They were loaded once at startup and then only when a sync changed the
  // status — so anything that altered them in between (a tag made in another
  // window, one created before the account finished opening, an edit that did
  // not refresh) left the picker showing a list that was merely the last one
  // seen. It is a local query against SQLite; asking again at the moment the
  // list is about to be read is cheaper than reasoning about when it went out
  // of date.
  useEffect(() => {
    if (picker !== 'tag') return;
    let live = true;
    // Reported, not swallowed. A tag list that failed to load and an account
    // with no tags produce exactly the same empty picker, so a silent catch
    // here turns a broken call into "you have no tags" — which is the one
    // reading of it that stops anyone looking for the real cause.
    api
      .tags()
      .then((t) => live && setTags(t))
      .catch((e) => api.log(`list_tags failed: ${e}`));
    return () => {
      live = false;
    };
  }, [picker, setTags]);

  const pickerOptions: PickerOption[] = useMemo(() => {
    // The same times snooze offers, for the same reason: "tomorrow" means the
    // start of a working day, not twenty-four hours from now.
    if (picker === 'snooze' || picker === 'send-later') return snoozeOptions();
    if (picker === 'tag') {
      const on = new Set((active?.tags ?? []).map((x) => x.name));
      const listed = new Set(tags.map((tg) => tg.name));
      // Whatever the conversation actually carries, even if the rail's list
      // does not have it. A tag visible on the message but absent from the
      // options is one the reader can see and cannot take off — the list being
      // briefly incomplete should not make a message impossible to untag.
      const carried = (active?.tags ?? []).filter((x) => !listed.has(x.name));
      return [...tags, ...carried].map((tg) => ({
        id: tg.id,
        label: tg.name,
        colour: tg.colour || undefined,
        on: on.has(tg.name),
      }));
    }
    // The place the conversation already is offers no move at all, so the
    // current view's folder leaves the list: in a folder view that folder,
    // in a role view the folder wearing that role.
    // Only folders the user made. The role mailboxes all have verbs of
    // their own (Archive, Trash, Spam, Move to Inbox), and offering their
    // raw server paths — [Gmail]/All Mail, INBOX — put rows in this list
    // that exist nowhere else in the app.
    // The Gmail anchor labels (a plain Archive or Trash the first nested
    // folder created) stay out: mail already has Archive and Trash as verbs,
    // and a folder row with the same name is the same place twice.
    const archiveAnchor = nestableRolePath(folders, 'archive');
    const trashAnchor = nestableRolePath(folders, 'trash');
    return folders
      .filter(
        (f) =>
          !f.role &&
          `folder:${f.id}` !== view &&
          f.path !== archiveAnchor &&
          f.path !== trashAnchor &&
          // Nothing files into a binned folder; fifty deleted alias folders
          // made the list unreadable before this.
          !underAnchor(f.path, trashAnchor),
      )
      .map((f) => ({ id: f.id, label: f.path }));
  }, [picker, folders, tags, active, view]);
  useEffect(() => {
    let live = true;
    setViewTotal(null);
    api
      .viewCount(view)
      .then((n) => live && setViewTotal(n))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [view, accountEpoch, status?.count]);

  // A message that needs a decision raises a notification, once.
  //
  // The amber rail is where you find out, but only if you are looking at the
  // rail. A message whose send could not be proved either way must not wait
  // silently behind a window you have minimised: silence is the one outcome
  // that loses mail. Raised on the count going *up*, so a message that has
  // already been announced is not announced again on every poll — and not
  // gated on the notification setting, because that setting governs mail
  // arriving, and this is mail of yours that may not have left.
  const announcedNeeds = useRef(0);
  useEffect(() => {
    const needs = counts['outbox:attention'] ?? 0;
    if (needs > announcedNeeds.current) {
      setToast(t('outbox-notify', { count: fmtCount(needs) }));
      void postDesktopNotification(t('outbox-notify-title'), t('outbox-notify', { count: fmtCount(needs) }));
    }
    announcedNeeds.current = needs;
  }, [counts]);

  // A `mailto:` in a message opens a message here rather than in whichever
  // other mail program the machine prefers. Web links go to the browser; that
  // decision lives in `useMessageLinks`.
  useMessageLinks(
    useCallback(
      (addr: string) => {
        attachmentWarned.current = false;
        setDraft({
          to: addr,
          cc: '',
          subject: '',
          body: startingBody(identity, false),
          html: startingHtml(identity, false),
        });
      },
      [identity],
    ),
    // A link whose spelling hides where it goes gets a question first. The
    // question names both spellings, because the whole trick is that one of
    // them is the one you read.
    useCallback((risk: HomographRisk, open: () => void) => {
      setRiskyLink({ risk, open });
    }, []),
  );

  // The rail's numbers come from the engine, not from the loaded page: counting
  // the rows in view told the inbox badge whatever the *current* view's unread
  // count was, so opening Spam relabelled the inbox with Spam's number.
  //
  // Recounted after every triage as well as every sync, because archiving the
  // last unread message and watching the badge keep its old number is the kind
  // of small lie that makes the whole rail untrustworthy.
  // Debounced, because the triggers arrive in bursts: an account switch
  // changes the epoch, the status, and the items within a few hundred
  // milliseconds, and each firing held the store lock for a third of a
  // second counting nine views — queued *ahead* of the thread list the
  // switch was actually for. One recount after the burst settles, and the
  // list's own queries take the lock first.
  useEffect(() => {
    let live = true;
    const t = window.setTimeout(() => {
      api
        .viewCounts(settings.badges)
        .then((rows) => live && setCounts(Object.fromEntries(rows)))
        .catch(() => {});
      // The switcher's per-account unread rides the same tick: it was read
      // only at sync boundaries, so it sat stale beside a mid-pane count
      // that moved with every triage.
      api
        .accounts()
        .then((a) => live && setAccounts(a))
        .catch(() => {});
    }, 300);
    return () => {
      live = false;
      window.clearTimeout(t);
    };
  }, [status?.count, status?.seeding, settings.badges, items, accountEpoch, setAccounts]);

  // First run: no account can sign in, so there is nothing to show but the
  // way to add one. Decided from the status the app reports, not from an
  // empty list — a mailbox that is merely empty is not a first run. And it
  // stays up until the person chooses "Start reading", so the first sync is
  // watched rather than happening behind a mailbox that looks broken.
  const [onboarded, setOnboarded] = useState(false);
  // Adding another account, from the switcher or from Settings. The same
  // three steps as a first run, in a dialog over the app.
  const [addingAccount, setAddingAccount] = useState(false);
  // Demo mode has a mailbox to show; onboarding is for a genuine first run.
  if (status && !status.configured && !status.demo && !onboarded) {
    return (
      <div className="app-frame">
        <TitleBar synced="" />
        <Onboarding onDone={() => setOnboarded(true)} />
      </div>
    );
  }

  return (
    <div className="app-frame">
      <TitleBar
        synced={((): string => {
          const sync = syncState(status);
          if (sync.kind === 'seeding') return t('status-seeding');
          if (sync.kind === 'failed') return t('titlebar-sync-failed');
          if (sync.kind === 'demo') return t('titlebar-demo');
          if (sync.kind === 'never') return t('titlebar-sync-waiting');
          return t('titlebar-sync');
        })()}
      />
      {status?.sync_error && (
        // Loud on purpose. A sync that fails silently is indistinguishable from
        // an account with no mail in it, and that ambiguity cost real time.
        <div className="sync-error" role="alert">
          <strong>{t('sync-failed-title')}</strong>
          <span>{status.sync_error}</span>
          <span className="sync-error-note">{t('sync-failed-body')}</span>
        </div>
      )}
      <div
        className="shell"
        data-layout={
          // Only while something is open. Filling the window with an empty
          // reading pane would hide the list to show nothing.
          readerFull && active && settings.layout !== 'off'
            ? 'reader-only'
            : settings.layout === 'off'
              ? 'no-reader'
              : settings.layout
        }
      >
      <Rail
        account={activeAccount?.email ?? status?.source ?? t('app-name')}
        accounts={accounts}
        accountColor={activeAccount?.color || 'var(--accent)'}
        unread={unread}
        counts={counts}
        outboxNeedsAttention={counts['outbox:attention'] ?? 0}
        view={view}
        folders={folders}
        onCreateFolder={(name) =>
          api
            .createFolder(name)
            .then(() => api.folders().then(setFolders))
            .then(() => setToast(t('folder-created', { name })))
            .catch((e) => setToast(t('folder-failed', { error: String(e) })))
        }
        onRenameFolder={(folderId, newPath) =>
          api
            .renameFolder(folderId, newPath)
            .then(() => api.folders().then(setFolders))
            .catch((e) => setToast(t('folder-failed', { error: String(e) })))
        }
        onDeleteFolder={setDeletingFolder}
        onMoveFolder={setMovingFolder}
        onEmptyTrash={() => setEmptyingTrash(true)}
        onDragFolder={startFolder}
        folderDragPath={
          drag?.payload.kind === 'folder'
            ? (folders.find(
                (f) => drag.payload.kind === 'folder' && f.id === drag.payload.folderId,
              )?.path ?? null)
            : null
        }
        onCreateTag={(name) =>
          api
            .createTag(name)
            // Re-read rather than push the new one in: the engine assigns the
            // colour, and a rail row invented here would be the wrong one until
            // the next refresh.
            .then(() => api.tags().then(setTags))
            .catch((e) => setToast(t('tag-create-failed', { error: String(e) })))
        }
        onRenameTag={(id, name) => {
          // The rows carry the tag's *name*, not its id, so a rename that only
          // refreshed the rail left every chip in the list showing the old one
          // until something else reloaded them.
          const was = tags.find((x) => x.id === id)?.name;
          return api
            .renameTag(id, name)
            .then(() => api.tags().then(setTags))
            .then(() => {
              if (!was || was === name) return;
              // A view is named after its tag, so renaming the tag you are
              // standing in leaves you looking at a name nothing answers to:
              // the list empties and no rail item is current. Follow the
              // rename instead — it is the same collection, newly titled.
              if (view === `tag:${was}`) setView(`tag:${name}`);
              setItems((prev) =>
                prev.map((row) =>
                  row.tags.some((x) => x.name === was)
                    ? {
                        ...row,
                        tags: row.tags.map((x) => (x.name === was ? { ...x, name } : x)),
                      }
                    : row,
                ),
              );
            })
            .catch((e) => setToast(t('tag-rename-failed', { error: String(e) })));
        }}
        onColourTag={(id, colour) => {
          // Painted at once and kept if the write succeeds. A colour is a
          // glance-level thing; waiting a round trip to see it is the whole
          // cost of the gesture.
          setTags((prev) => prev.map((x) => (x.id === id ? { ...x, colour } : x)));
          void api
            .setTagColour(id, colour)
            .then(() => api.tags().then(setTags))
            .catch((e) => setToast(t('tag-rename-failed', { error: String(e) })));
        }}
        onDeleteTag={(tag) => setDeletingTag(tag)}
        onDragTag={startTag}
        tags={tags}
        railRef={railRef}
        collapsed={settings.railCollapsed === 'on'}
        onCompose={() => {
          attachmentWarned.current = false;
          setDraft({
            to: '',
            cc: '',
            subject: '',
            body: startingBody(identity, false),
            html: startingHtml(identity, false),
          });
        }}
        onToggleCollapsed={() =>
          set('railCollapsed', settings.railCollapsed === 'on' ? 'off' : 'on')
        }
        onResize={(xOrDelta) => {
          // A pointer gives an absolute x; the keyboard gives a delta. The rail
          // starts at the window edge, so its width *is* the pointer's x.
          const next =
            Math.abs(xOrDelta) < 64 && xOrDelta !== 0
              ? clampRail(settings.railWidth) + xOrDelta
              : xOrDelta;
          set('railWidth', String(clampRail(next)));
        }}
        // The same call the ⌘1…9 keys make. This was a stub that only
        // showed the toast, so the menu said "Switched to…" and switched
        // nothing while the keys worked — the button and the key had drifted.
        onSwitchAccount={(n) => {
          const acc = accounts[n - 1];
          if (!acc) {
            setToast(t('account-none-at', { n: String(n) }));
            return;
          }
          if (!acc.active) void switchAccount(acc.id, acc.email);
        }}
        onSettings={() => setSettingsOpen('accounts')}
        onAddAccount={() => setAddingAccount(true)}
        dropOver={drag?.over ?? null}
        insertAt={drag?.insert ?? null}
        // Only while conversations are in flight. A tag being carried can only
        // land on a conversation, so lighting up every mailbox would be
        // offering somewhere it cannot go.
        dragActive={drag?.payload.kind === 'threads'}
        onView={(v) => {
          if (v === 'help') setHelpOpen(true);
          else if (v === 'settings') setSettingsOpen('appearance');
          else goToView(v);
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
              // Same reason as the palette: the webview's autofill menu is not
              // ours, and it covers the results underneath it.
              autoComplete="off"
              autoCorrect="off"
              spellCheck={false}
              value={query}
              placeholder={t('search-placeholder')}
              onChange={(e) => setQuery(e.target.value)}
              // The context is applied the moment the field is entered — the
              // pills are already showing, and the token they mirror should
              // be too. Typed after that, never re-applied: deleting the
              // token is how a search goes global.
              onFocus={() => {
                setSearching(true);
                if (query.trim()) return;
                const leaf = folderLeaf(view, folders);
                const scope = scopeFor(view, leaf);
                if (scope) setQuery(`${scope.token} `);
              }}
              onKeyDown={(e) => {
                // Escape leaves search rather than merely leaving the field:
                // a blurred box still holding a query is still a search, with
                // the results and highlights to prove it.
                if (e.key === 'Escape') {
                  e.stopPropagation();
                  setQuery('');
                  e.currentTarget.blur();
                }
              }}
              // Kept up while a chip is being clicked: blur fires first, and
              // hiding the row on the way to it would move the target away.
              onBlur={() => {
                // Leaving with only the pre-applied token means no search was
                // meant; the field empties rather than staying half-armed.
                const leaf = folderLeaf(view, folders);
                if (query.trim() === scopeFor(view, leaf)?.token) setQuery('');
                window.setTimeout(() => setSearching(false), 150);
              }}
              aria-label={t('search-placeholder')}
            />
            <span className="kbd">{t('search-hint-key')}</span>
          </div>

          {/* Chips write into the field rather than filtering beside it. Each
              is lit because its token is in the query — type `is:unread` by
              hand and the chip lights, which is the point: there is one place
              the search lives and it is the one you can see. */}
          {(searching || query.trim()) && (
            <div className="chip-row" role="group" aria-label={t('search-filters')}>
              {chips(
                active?.from_display || active?.from_addr || null,
                new Date().getFullYear(),
                view,
                folderLeaf(view, folders),
              )
                .map((c) => (
                  <button
                    key={c.id}
                    type="button"
                    className={hasToken(query, c.token) ? 'filter-chip on' : 'filter-chip'}
                    aria-pressed={hasToken(query, c.token)}
                    onClick={() => setQuery(toggleToken(query, c.token))}
                  >
                    {c.label}
                  </button>
                ))}
            </div>
          )}
          {/* No account chip here. One account is active at a time and the
              rail names it, so this repeated it a few inches away — and its dot
              was painted with the theme accent rather than the account's own
              colour, so two accounts in different colours would have shown the
              same dot. It earns a place again the day a view can hold mail from
              more than one account. */}
          <div className="view-row">
            <span className="view-name">{viewName}</span>
            {/* Searching is a different question from browsing, so the header
                answers a different one: how many were found, and in what
                order — not how many are unread. */}
            {query.trim() ? (
              <>
                <span className="view-count">
                  {t('search-found', { count: fmtCount(items.length) })}
                </span>
                {/* No auto margin here: the count already claims the free
                    space, and two elements both pushing left split it between
                    them — which left "N found" adrift in the middle of the row
                    rather than sitting with the control it belongs to. */}
                <div className="sort-row tight" role="group" aria-label={t('search-sort')}>
                  <button
                    type="button"
                    className={newestFirst ? undefined : 'on'}
                    aria-pressed={!newestFirst}
                    onClick={() => setNewestFirst(false)}
                  >
                    {t('search-sort-best')}
                  </button>
                  <button
                    type="button"
                    className={newestFirst ? 'on' : undefined}
                    aria-pressed={newestFirst}
                    onClick={() => setNewestFirst(true)}
                  >
                    {t('search-sort-newest')}
                  </button>
                </div>
              </>
            ) : (
              <span className="view-count">
                {selected.size > 0
                  ? t('list-selected', { count: fmtCount(selected.size) })
                  : view === 'outbox'
                    ? // "Unread" means nothing for mail you wrote. What the outbox
                      // counts is what is waiting, and what is waiting on you.
                      (counts['outbox:attention'] ?? 0) > 0
                      ? t('outbox-count-attention', {
                          count: fmtCount(counts['outbox'] ?? 0),
                          needs: fmtCount(counts['outbox:attention'] ?? 0),
                        })
                      : t('outbox-count', { count: fmtCount(counts['outbox'] ?? 0) })
                    : t('list-unread', { count: fmtCount(unread) })}
              </span>
            )}
          </div>
        </div>

        {error ? (
          <div className="empty">
            <h2 style={{ color: 'var(--danger)' }}>Could not load this mailbox</h2>
            <p className="mono" style={{ fontSize: 11.5 }}>{error}</p>
          </div>
        ) : view === 'outbox' && !query.trim() ? (
          // Its own component, not conversation rows. An outbox row is a
          // message in one of five states, and the row's job is to say which
          // in plain words and offer only the actions that state allows.
          <Outbox onDiscard={(row) => setDiscarding(row)} />
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
            selected={selected}
            onToggleSelect={(id) => {
              setSelected((cur) => toggle(cur, id));
              setAnchor(id);
            }}
            density={settings.density}
            checkboxes={settings.checkboxes === 'on'}
            onActivate={(id, mods) => {
              // Cmd/ctrl adds or removes one; shift reaches back to where the
              // selection started. Neither opens the conversation — picking
              // several and having the last one fill the reading pane is how
              // you accidentally mark something read while gathering a batch.
              if (mods.toggle) {
                setSelected((cur) => toggle(cur, id));
                setAnchor(id);
                return;
              }
              if (mods.range) {
                setSelected((cur) => extend(cur, items.map((m) => m.id), anchor, id));
                return;
              }
              // A plain click is "just this one", so it puts a selection away
              // rather than quietly acting on rows still held from before.
              if (selected.size > 0) setSelected(new Set());
              setAnchor(id);
              setActiveId(id);
              // In Drafts, selecting one means resuming it. Showing an
              // unfinished message in a reading pane is showing it to the
              // person who wrote it, in the one form they cannot edit.
              if (view === 'drafts') void resumeDraft(id);
            }}
            onAction={(kind, threadId) => void triage.run(kind, threadId)}
            onSnooze={(threadId) => {
              setActiveId(threadId);
              setPicker('snooze');
            }}
            onDragStart={startDrag}
            dropRow={drag?.overRow ?? null}
            onContextMenu={(id, x, y) => {
              // Right-clicking inside a selection acts on all of it; on a row
              // outside one, the selection is dropped and only that row is in
              // play. Anything else and the menu would quietly act on rows the
              // user was no longer pointing at.
              if (!selected.has(id)) {
                setSelected(new Set());
                setAnchor(null);
                setActiveId(id);
              }
              setRowMenu({ id, x, y });
            }}
          />
        )}

        {/* What was actually searched. During a backfill the index genuinely
            does not hold everything, and a client that quietly returns three
            results out of a possible ten teaches you not to trust its search.
            Saying so keeps "no results" meaning no results.

            Shown only when the two numbers disagree: once everything is held,
            a line explaining that everything was searched is noise. */}
        {query.trim() &&
          (status?.server_total ?? 0) > (status?.count ?? 0) && (
            <div className="coverage">
              {t('search-coverage', {
                searched: fmtCount(status?.count ?? 0),
                total: fmtCount(status?.server_total ?? 0),
              })}
              {status?.seeding ? ` ${t('search-coverage-syncing')}` : ''}
            </div>
          )}
      </div>

      {/* Only where there are two panes side by side to divide. Stacked, the
          boundary is horizontal and this handle would resize the wrong axis;
          with the reader off or filling the window there is no boundary. */}
      {settings.layout === 'right' && !readerFull && (
        <PaneResize
          onResize={(xOrDelta) => {
            // A pointer gives an absolute x; the keyboard gives a delta. The
            // list does not start at the window edge, so unlike the rail its
            // width is the pointer's x less whatever the rail is occupying.
            const railNow =
              settings.railCollapsed === 'on' ? RAIL_COLLAPSED : clampRail(settings.railWidth);
            const next =
              Math.abs(xOrDelta) < 64 && xOrDelta !== 0
                ? clampList(settings.listWidth) + xOrDelta
                : xOrDelta - railNow;
            set('listWidth', String(clampList(next)));
          }}
        />
      )}

      {(settings.layout !== 'off' || readerOverlay) && (
        <Reader
          thread={active}
          view={view}
          onToast={setToast}
          onComposeMailto={(to, subject) => {
            attachmentWarned.current = false;
            setDraft({
              to,
              cc: '',
              subject,
              body: startingBody(identity, false),
              html: startingHtml(identity, false),
            });
          }}
          onReplyTo={(messageId, all) => {
            if (active) void startReply(active.id, all, messageId);
          }}
          onForwardFrom={(messageId) => {
            if (active) void startForward(active.id, messageId);
          }}
          full={readerFull}
          finding={finding}
          onCloseFind={() => setFinding(false)}
          onToggleFull={() => setReaderFull((f) => !f)}
          onPopOut={() => {
            if (!active) return;
            void api
              .popoutMessage(active.thread_id)
              .catch((e) => setToast(t('popout-failed', { error: String(e) })));
          }}
          onAction={(kind) => (kind === 'delete_forever' ? askDelete() : void triage.run(kind))}
          onMove={() => setPicker('folder')}
          onMoveInbox={() => {
            const inbox = folders.find((f) => f.role === 'inbox');
            if (inbox) void triage.run('move', undefined, inbox.id);
          }}
          onTag={() => setPicker('tag')}
          onSnooze={() => setPicker('snooze')}
        />
      )}

      {addingAccount && (
        <Dialog
          open
          onClose={() => setAddingAccount(false)}
          // The dimming is the dialog container's own background, not a
          // separate backdrop element. Ariakit portals a backdrop as a sibling
          // of the dialog, and in the stacking order that produced, the scrim
          // either sat beneath Settings or painted over the card — there was
          // no z-index that put it between the two. The container is one
          // element that is certainly above Settings and certainly below its
          // own child, which is the only ordering that can be relied on.
          className="onboarding-dialog"
          backdrop={false}
        >
          <div
            className="onboarding-dim"
            onClick={(e) => {
              if (e.target === e.currentTarget) setAddingAccount(false);
            }}
          >
            <Onboarding
              onDone={(added) => {
                setAddingAccount(false);
                // You just walked three screens to add it: show it. Adding
                // an account and then leaving the old one on screen read as
                // the add having failed, while the new mail synced unseen.
                if (added != null) void switchAccount(added.id, added.email);
                else setAccountEpoch((n) => n + 1);
              }}
            />
          </div>
        </Dialog>
      )}

      <DragPreview drag={drag} />

      {draft && (
        <Compose
          draft={draft}
          account={activeAccount?.email ?? ''}
          onChange={setDraft}
          onClose={() => {
            // Keeping it, not discarding it. Losing what someone wrote because
            // they hit the wrong corner is unforgivable, and a confirmation on
            // every close is worse than simply keeping the message.
            // CC and attachments count as content too. They did not, and a
            // draft that was only a CC line or only an attached file was
            // dropped on close without a word — the exact loss the comment
            // above is about.
            if (
              draft.to ||
              draft.cc ||
              draft.subject ||
              draft.body.trim() ||
              (draft.attachments?.length ?? 0) > 0
            )
              void saveDraft(draft).then((id) => {
                // Closing must not wait out the 30-second debounce.
                if (id != null) void api.pushDraft(id).catch(() => {});
              });
            setDraft(null);
          }}
          onAttach={() => void attach()}
          onDropFiles={(files) => void dropAttachments(files)}
          onSaveDraft={() => void saveDraft(draft)}
          onSendLater={() => setPicker('send-later')}
          onNotice={setToast}
          onPopOut={() => {
            // Saved first: the new window is given an id, and that id is also
            // what stops the two windows becoming separate unsaved copies of
            // the same message.
            const d = draft;
            void saveDraft(d)
              .then(() => api.popoutCompose(draftRef.current?.savedId ?? d.savedId ?? 0))
              .then(() => setDraft(null))
              .catch((e) => setToast(t('compose-popout-failed', { error: String(e) })));
          }}
          onSend={() => {
            if (addresses(draft.to).length === 0) {
              setToast(t('compose-no-recipient'));
              return;
            }
            // Asked once. A second Send goes, because a warning you cannot get
            // past is a bug rather than a safeguard — sometimes you really did
            // mean to write "as attached" about something sent last week.
            if (
              settings.warnMissingAttachment === 'on' &&
              !attachmentWarned.current &&
              promisesMissingAttachment(draft.subject, draft.body, draft.attachments?.length ?? 0)
            ) {
              attachmentWarned.current = true;
              setToast(t('compose-missing-attachment'));
              return;
            }
            attachmentWarned.current = false;
            // Into the outbox, never straight onto the wire. The undo window
            // is the message sitting in the outbox with its time set a few
            // seconds out — so closing the app mid-countdown does not lose it
            // (it goes when the app is next open), a failed send lands in the
            // outbox with its reason rather than as a toast, and the
            // ambiguous-outcome rule protects every send, not only the
            // scheduled ones.
            const wait = Number(settings.undoSendSeconds) || 0;
            const d = draft;
            setDraft(null);
            void api
              .saveDraft(d.savedId ?? null, d.to, d.subject, d.body, d.html, envelopeOf(d))
              .then((id) => api.scheduleSend(id, Date.now() + wait * 1000).then(() => id))
              .then((id) => setOutgoing({ id, subject: d.subject, left: wait }))
              .catch((e) => {
                // Could not even queue it: the draft comes back. Losing what
                // someone wrote is the one failure that is unforgivable.
                setDraft(d);
                setToast(t('compose-failed', { error: String(e) }));
              });
          }}
        />
      )}

      <Picker
        // Folders are labels only on Gmail. Telling a Fastmail or Exchange user
        // about label behaviour is telling them something false about their own
        // mail, so the note is shown to the accounts it describes.
        labelsNotFolders={activeAccount?.kind === 'gmail'}
        open={picker !== null}
        mode={picker === 'send-later' ? 'snooze' : (picker ?? 'folder')}
        subject={active?.subject ?? null}
        options={pickerOptions}
        onClose={() => setPicker(null)}
        onChoose={(id, on) => {
          if (picker === 'send-later') {
            // Saved first: a message cannot wait in the outbox unless it
            // exists there, and the id is what the schedule hangs on.
            const d = draftRef.current;
            setPicker(null);
            if (!d) return;
            void api
              .saveDraft(d.savedId ?? null, d.to, d.subject, d.body, d.html)
              .then((saved) => api.scheduleSend(saved, id))
              .then(() => {
                setDraft(null);
                setToast(t('compose-scheduled', { when: new Date(id).toLocaleString() }));
              })
              .catch((e) => setToast(t('compose-save-failed', { error: String(e) })));
            return;
          }
          if (picker === 'snooze') {
            // The id *is* the instant to come back at — a snooze has no row to
            // point at, only a time.
            void triage.run('snooze', undefined, id);
            setPicker(null);
            return;
          }
          // Whatever is selected, or the conversation on screen when nothing
          // is. Passing `undefined` here meant the picker always acted on the
          // active row alone, so tagging six selected conversations tagged one
          // of them and silently left the rest — the selection was visible on
          // screen the whole time.
          const targets = selected.size > 0 ? [...selected] : [undefined];
          if (picker === 'folder') {
            targets.forEach((t) => void triage.run('move', t, id));
            setPicker(null);
            if (selected.size > 0) setSelected(new Set());
          } else {
            // Toggling: `on` is the state being moved to, so an applied tag
            // untags rather than re-applying and reporting "Tagged" twice.
            targets.forEach((t) => void triage.run(on ? 'tag' : 'untag', t, id));
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
              // Re-read the tags *before* applying, not after. The row shows a
              // tag by name and colour, which the optimistic patch looks up by
              // id against the loaded list — and a tag created a moment ago is
              // not in that list yet, so applying first left the row bare until
              // something else reloaded it.
              return api.tags().then(setTags).then(() => {
                // ...and to everything selected, not only the row underneath.
                const targets = selected.size > 0 ? [...selected] : [undefined];
                return Promise.all(
                  targets.map((target) => triage.run('tag', target, id)),
                ).then(() => undefined);
              });
            })
            .catch((e) => setToast(t('triage-failed', { error: String(e) })));
        }}
      />

      <Palette
        open={paletteOpen}
        onOpen={(threadId: number) => {
          // Opening from the palette puts the conversation in the reading pane
          // if the current view already holds it. If it does not, the query is
          // what puts it there — jumping the list to a row it does not contain
          // would land on nothing.
          const row = items.find((m) => m.thread_id === threadId);
          if (row) setActiveId(row.id);
        }}
        onClose={() => setPaletteOpen(false)}
        subject={active?.subject ?? null}
        ctx={{
          hasThread: !!active,
          // Every one of these is the same call the keyboard makes. The palette
          // finds a command; it does not reimplement one.
          onAction: (kind) => void triage.run(kind),
          onSnooze: () => setPicker('snooze'),
          onMove: () => setPicker('folder'),
          onTag: () => setPicker('tag'),
          onCompose: startCompose,
          onReply: () => {
            if (active) void startReply(active.id, settings.replyDefault === 'reply-all');
          },
          onPauseNotifications: () => {
            set('notifyPausedUntil', String(Date.now() + 60 * 60 * 1000));
            setToast(t('notify-paused-toast'));
          },
          onView: (v) => {
            if (v === 'help') setHelpOpen(true);
            else if (v === 'settings') setSettingsOpen('appearance');
            else if (v === 'search') searchRef.current?.focus();
            else setView(v);
          },
        }}
      />
      <Help open={helpOpen} onClose={() => setHelpOpen(false)} />
      <Settings
        onAddAccount={() => setAddingAccount(true)}
        open={settingsOpen !== null}
        pane={settingsOpen ?? undefined}
        onClose={() => {
          setSettingsOpen(null);
          api.accounts().then(setAccounts).catch(() => {});
        }}
        onMessage={setToast}
      />
      {outgoing && (
        // Its own bar, not the toast: this one is a control with a deadline,
        // and burying it in the same channel as "Archived" invites missing it.
        <div className="sending" role="status">
          <span className="sending-count mono">{outgoing.left}s</span>
          <span className="clip">
            {t('compose-sending', { count: outgoing.left })}: {outgoing.subject || t('no-subject')}
          </span>
          {/* The same path as Z, so the button and the key cannot drift. */}
          <button type="button" className="reply" onClick={cancelPendingSend}>
            {t('undo')} <span className="kbd">Z</span>
          </button>
          {/* Tells the outbox, not only the counter: zeroing the clock here
              would end the toast while the message sat waiting out its
              window in the store. */}
          <button
            type="button"
            className="sending-now"
            onClick={() => {
              const o = outgoing;
              setOutgoing({ ...o, left: 0 });
              void api.outboxSendNow(o.id).catch((e) => setToast(t('compose-failed', { error: String(e) })));
            }}
          >
            {t('compose-send-now')}
          </button>
        </div>
      )}

      {rowMenu &&
        (() => {
          const row = items.find((m) => m.thread_id === rowMenu.id || m.id === rowMenu.id);
          if (!row) return null;
          const targets = selected.has(rowMenu.id) ? [...selected] : [rowMenu.id];
          const close = () => setRowMenu(null);
          // Every item routes through the same paths the rest of the app uses,
          // so a right-click cannot become a second, subtly different way to
          // archive something.
          return (
            <RowMenu
              at={{ x: rowMenu.x, y: rowMenu.y }}
              onClose={close}
              thread={row}
              view={view}
              count={targets.length}
              onAction={(kind) => {
                close();
                if (kind === 'delete_forever') {
                  askDelete(targets);
                  return;
                }
                targets.forEach((id) => void triage.run(kind, id));
                if (selected.size > 0) setSelected(new Set());
              }}
              onMoveInbox={() => {
                close();
                const inbox = folders.find((f) => f.role === 'inbox');
                if (!inbox) return;
                targets.forEach((id) => void triage.run('move', id, inbox.id));
                if (selected.size > 0) setSelected(new Set());
              }}
              onMove={() => {
                close();
                setPicker('folder');
              }}
              onTag={() => {
                close();
                setPicker('tag');
              }}
              onSnooze={() => {
                close();
                setPicker('snooze');
              }}
              onPopOut={() => {
                close();
                void api
                  .popoutMessage(row.thread_id)
                  .catch((e) => setToast(t('popout-failed', { error: String(e) })));
              }}
            />
          );
        })()}

      <AppDialogs
        discarding={discarding}
        setDiscarding={setDiscarding}
        deletingTag={deletingTag}
        setDeletingTag={setDeletingTag}
        movingFolder={movingFolder}
        setMovingFolder={setMovingFolder}
        deletingFolder={deletingFolder}
        setDeletingFolder={setDeletingFolder}
        pendingDelete={pendingDelete}
        setPendingDelete={setPendingDelete}
        draftConflict={draftConflict}
        onSettleDraftConflict={(take) => void settleDraftConflict(take)}
        riskyLink={riskyLink}
        onDismissRiskyLink={() => setRiskyLink(null)}
        emptyingTrash={emptyingTrash}
        onCancelEmptyTrash={() => setEmptyingTrash(false)}
        onEmptyTrash={() => {
          setEmptyingTrash(false);
          void api
            .emptyTrash()
            .then((r) => {
              const [gone, kept] = r.split('/');
              setAccountEpoch((n) => n + 1);
              setToast(
                Number(gone) === 0 && Number(kept) === 0
                  ? t('trash-already-empty')
                  : Number(kept) > 0
                    ? t('trash-emptied-partial', { count: gone, kept })
                    : t('trash-emptied', { count: gone }),
              );
            })
            .catch((e) => setToast(t('trash-empty-failed', { error: String(e) })));
        }}
        view={view}
        setView={setView}
        folders={folders}
        setFolders={setFolders}
        setTags={setTags}
        setToast={setToast}
        items={items}
        selectedSize={selected.size}
        clearSelected={() => setSelected(new Set())}
        runTriage={(kind, threadId, targetId, quiet) =>
          void triage.run(kind, threadId, targetId, quiet)
        }
        clearUndo={() => setUndoOffer(null)}
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
          {((): string => {
            const sync = syncState(status);
            if (sync.kind === 'seeding') return t('status-seeding');
            if (sync.kind === 'failed') return t('status-sync-failed');
            // Nothing is on its way, so nothing is being waited for.
            if (sync.kind === 'demo') return t('status-demo');
            if (sync.kind === 'never') return t('status-sync-waiting');
            // Aged from a real timestamp; the old label was a constant
            // and therefore eternally "just now".
            const min = Math.floor((Date.now() - sync.at) / 60000);
            if (min < 2) return t('status-synced');
            if (min < 120) return t('status-synced-min', { min: String(min) });
            return t('status-synced-hr', { hr: String(Math.floor(min / 60)) });
          })()}
        </span>
        {/* A rule between two facts, drawn as a character. Hidden from
            assistive technology because "pipe" between them is noise, and
            exempt from contrast for the same reason it is faint: it is a
            divider, not something to read. */}
        <span style={{ color: 'var(--hair)' }} aria-hidden="true">
          |
        </span>
        <span>
          {view === 'outbox'
            ? t('outbox-count', { count: fmtCount(counts['outbox'] ?? 0) })
            : t('status-counts', { count: fmtCount(viewTotal ?? items.length), unread: fmtCount(unread) })}
        </span>
        <span className="spacer" />
        <span>
          <span className="kbd">J</span> <span className="kbd">K</span> move
        </span>
        <span>
          <span className="kbd">/</span> search
        </span>
        <span>
          <span className="kbd">⌘K</span> {t('palette-title')}
        </span>
      </footer>
      </div>
    </div>
  );
}
