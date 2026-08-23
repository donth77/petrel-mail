import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  api,
  type Account,
  type Folder,
  type Identity,
  type Status,
  type Tag,
  type Thread,
} from './lib/api';
import { chips, hasToken, scopedQuery, toggleToken } from './lib/search-chips';
import { count as fmtCount, fileSize } from './lib/format';
import { t, type StringId } from './lib/strings';
import { Search } from 'lucide-react';
import { Rail } from './components/Rail';
import { useKeyboard } from './lib/useKeyboard';
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
import { startingBody, startingHtml } from './lib/signature';
import { ATTACHMENT_LIMIT, pickAttachments, stageDropped } from './lib/attachments';
import { extend, prune, targets, toggle } from './lib/selection';
import { notifiable, postDesktopNotification } from './lib/notify';
import { Help } from './components/Help';
import { Settings } from './components/Settings';
import { RAIL_COLLAPSED, clampList, clampRail, useSettings } from './lib/settings';
import { useMessageLinks } from './lib/links';
import { Confirm } from './components/Confirm';
import { RowMenu } from './components/RowMenu';
import { Toast } from './components/Toast';
import { MessageList } from './components/MessageList';
import { Reader } from './components/Reader';
import { PaneResize } from './components/PaneResize';

export function App() {
  const { settings, set } = useSettings();
  const [status, setStatus] = useState<Status | null>(null);
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
  // Conversations currently being dragged, so the rail can show which
  // destinations will take them.
  const [dragging, setDragging] = useState<number[]>([]);
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
  const [outgoing, setOutgoing] = useState<{ draft: Draft; left: number } | null>(null);
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
  }, [query, view, newestFirst, status?.count, status?.seeding]);

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
        setDraft(outgoing.draft);
        setOutgoing(null);
        setToast(t('compose-cancelled'));
        return;
      }
      void triage.undo();
    },
    switchAccount: (n) => {
      const acc = accounts[n - 1];
      if (acc) setToast(t('account-switched', { email: acc.email }));
      else setToast(t('account-none-at', { n: String(n) }));
    },
    compose: () => {
      attachmentWarned.current = false;
      setDraft({
        to: '',
        cc: '',
        subject: '',
        body: startingBody(identity, false),
        html: startingHtml(identity, false),
      });
    },
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
  }, [query, view, viewName, status?.count]);

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
    } catch (e) {
      setToast(t('compose-resume-failed', { error: String(e) }));
    }
  };

  /**
   * Saves the composer's contents, and remembers the id so saving again
   * updates the same draft rather than leaving a trail of near-identical ones.
   */
  const saveDraft = async (d: Draft) => {
    try {
      const id = await api.saveDraft(d.savedId ?? null, d.to, d.subject, d.body, d.html);
      setDraft((cur) => (cur ? { ...cur, savedId: id } : cur));
      setToast(t('compose-saved'));
      // The Drafts view is a query, so it only changes when the list reloads.
      if (view === 'drafts') api.threads(view, 0, 500).then(setItems).catch(() => {});
    } catch (e) {
      setToast(t('compose-save-failed', { error: String(e) }));
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

  // The undo-send countdown. Nothing has been sent while this runs.
  useEffect(() => {
    if (!outgoing) return;
    if (outgoing.left <= 0) {
      const d = outgoing.draft;
      setOutgoing(null);
      void api
        .send(
          addresses(d.to),
          addresses(d.cc),
          d.subject,
          d.body,
          d.html || null,
          d.inReplyTo ?? null,
          d.references ?? [],
          (d.attachments ?? []).map((a) => a.path),
        )
        .then(() => {
          // It has gone; leaving it in Drafts would offer to send it twice.
          if (d.savedId != null) void api.deleteDraft(d.savedId).catch(() => {});
          setToast(t('compose-sent'));
        })
        .catch((e) => {
          // The draft comes back rather than evaporating: a failed send that
          // loses what you wrote is unforgivable, and the error text is often
          // something only the writer can act on.
          setDraft(d);
          setToast(t('compose-failed', { error: String(e) }));
        });
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

  // Tags come from the account, so one that has no conversation on this page
  // still appears in the rail.
  const [tags, setTags] = useState<Tag[]>([]);
  const [counts, setCounts] = useState<Record<string, number>>({});
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [identity, setIdentity] = useState<Identity | null>(null);

  // Folders show their full path so "Contracts/2026" is distinguishable from
  // another "2026" elsewhere; tags carry their colour and whether this
  // conversation already has them, because tagging is a set, not a choice.
  const pickerOptions: PickerOption[] = useMemo(() => {
    // The same times snooze offers, for the same reason: "tomorrow" means the
    // start of a working day, not twenty-four hours from now.
    if (picker === 'snooze' || picker === 'send-later') return snoozeOptions();
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
    api.identity().then((i) => live && setIdentity(i)).catch((e) => api.log(`identity failed: ${e}`));
    api.accounts().then((a) => live && setAccounts(a)).catch(() => {});
    return () => {
      live = false;
    };
  }, [status?.count, status?.seeding]);

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
  );

  // The rail's numbers come from the engine, not from the loaded page: counting
  // the rows in view told the inbox badge whatever the *current* view's unread
  // count was, so opening Spam relabelled the inbox with Spam's number.
  //
  // Recounted after every triage as well as every sync, because archiving the
  // last unread message and watching the badge keep its old number is the kind
  // of small lie that makes the whole rail untrustworthy.
  useEffect(() => {
    let live = true;
    api
      .viewCounts(settings.badges)
      .then((rows) => live && setCounts(Object.fromEntries(rows)))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [status?.count, status?.seeding, settings.badges, items]);

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
        account={accounts[0]?.email ?? status?.source ?? t('app-name')}
        accounts={accounts}
        accountColor={accounts[0]?.color || 'var(--accent)'}
        unread={unread}
        counts={counts}
        view={view}
        onCreateTag={(name) =>
          api
            .createTag(name)
            // Re-read rather than push the new one in: the engine assigns the
            // colour, and a rail row invented here would be the wrong one until
            // the next refresh.
            .then(() => api.tags().then(setTags))
            .catch((e) => setToast(t('tag-create-failed', { error: String(e) })))
        }
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
        onSwitchAccount={(n) => {
          const acc = accounts[n - 1];
          if (acc) setToast(t('account-switched', { email: acc.email }));
          else setToast(t('account-none-at', { n: String(n) }));
        }}
        onSettings={() => setSettingsOpen('accounts')}
        onDropThreads={dropOnRail}
        dragActive={dragging.length > 0}
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
              onChange={(e) => setQuery(scopedQuery(e.target.value, query, view))}
              onFocus={() => setSearching(true)}
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
              onBlur={() => window.setTimeout(() => setSearching(false), 150)}
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
              {chips(active?.from_display || active?.from_addr || null, new Date().getFullYear(), view)
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
            onDragIds={setDragging}
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
          onTag={() => setPicker('tag')}
          onSnooze={() => setPicker('snooze')}
        />
      )}

      {draft && (
        <Compose
          draft={draft}
          account={accounts[0]?.email ?? ''}
          onChange={setDraft}
          onClose={() => {
            // Keeping it, not discarding it. Losing what someone wrote because
            // they hit the wrong corner is unforgivable, and a confirmation on
            // every close is worse than simply keeping the message.
            if (draft.to || draft.subject || draft.body.trim()) void saveDraft(draft);
            setDraft(null);
          }}
          onAttach={() => void attach()}
          onDropFiles={(files) => void dropAttachments(files)}
          onSaveDraft={() => void saveDraft(draft)}
          onSendLater={() => setPicker('send-later')}
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
            const wait = Number(settings.undoSendSeconds) || 0;
            setOutgoing({ draft, left: wait });
            setDraft(null);
          }}
        />
      )}

      <Picker
        // Folders are labels only on Gmail. Telling a Fastmail or Exchange user
        // about label behaviour is telling them something false about their own
        // mail, so the note is shown to the accounts it describes.
        labelsNotFolders={accounts[0]?.kind === 'gmail'}
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
          onCompose: () =>
            setDraft({
              to: '',
              cc: '',
              subject: '',
              body: startingBody(identity, false),
              html: startingHtml(identity, false),
            }),
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
            {t('compose-sending', { count: String(outgoing.left) })} — {outgoing.draft.to}
          </span>
          <button type="button" className="reply" onClick={() => { setDraft(outgoing.draft); setOutgoing(null); setToast(t('compose-cancelled')); }}>
            {t('undo')} <span className="kbd">Z</span>
          </button>
          <button type="button" className="sending-now" onClick={() => setOutgoing({ ...outgoing, left: 0 })}>
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

      <Confirm
        open={pendingDelete !== null}
        title={t('delete-forever-confirm')}
        detail={
          pendingDelete?.length === 1
            ? t('delete-forever-one', {
                subject:
                  // Either kind of id can be in here — the keyboard path
                  // carries the active message id and the row path a thread
                  // id — so match the way triage itself does.
                  items.find(
                    (m) => m.id === pendingDelete[0] || m.thread_id === pendingDelete[0],
                  )?.subject || t('no-subject'),
              })
            : t('delete-forever-many', { count: fmtCount(pendingDelete?.length ?? 0) })
        }
        confirmLabel={t('delete-forever')}
        onClose={() => setPendingDelete(null)}
        onConfirm={() => {
          const ids = pendingDelete ?? [];
          setPendingDelete(null);
          // No undo offered, because there is none to offer. The toast reports
          // what happened and stops there.
          ids.forEach((id) => void triage.run('delete_forever', id, undefined, true));
          if (selected.size > 0) setSelected(new Set());
          // Clear the standing offer before saying anything. The toast is one
          // surface: leaving the previous action's Undo attached to this
          // message puts an undo button on a permanent delete, which is the
          // precise lie the confirmation dialog exists to avoid.
          setUndoOffer(null);
          setToast(t('deleted-forever'));
        }}
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
          <span className="kbd">⌘K</span> {t('palette-title')}
        </span>
      </footer>
      </div>
    </div>
  );
}
