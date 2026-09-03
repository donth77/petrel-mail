import { useCallback, useRef, useState } from 'react';
import { api, type ActionKind, type Thread } from './api';
import { countDeltas } from './counts';
import type { CountMode } from './mailboxes';
import { t } from './strings';

/** Whether an action takes a conversation out of the list you are looking at.
 *
 *  This cannot be read off the action alone. Archiving removes a row from the
 *  inbox and from trash, but not from the archive; unstarring removes it from
 *  Starred and from nowhere else. Deciding by action alone left rows sitting in
 *  lists they no longer belonged to. */
function leavesView(kind: ActionKind, view: string): boolean {
  // Filed somewhere specific, or gone entirely: either way it is not here.
  if (kind === 'move' || kind === 'delete_forever') return true;

  // Trash and spam are exclusive placements on both kinds of provider — the
  // conversation leaves wherever it was. So it leaves whatever list you happen
  // to be looking at, unless that list is where it lands.
  //
  // This used to be enumerated view by view, and the enumeration was wrong:
  // Sent, Drafts, Snoozed and every tag view were all listed as places nothing
  // moves out of, so binning something from any of them left the row sitting
  // there until a refresh took it away.
  if (kind === 'trash') return view !== 'trash';
  if (kind === 'spam') return view !== 'spam';

  // Out of the inbox, and out of a bin it is being rescued from. Stars and tags
  // survive archiving, so those views keep the conversation.
  if (kind === 'archive') return view === 'inbox' || view === 'trash' || view === 'spam';

  if (kind === 'snooze') return view === 'inbox';
  if (kind === 'unsnooze') return view === 'snoozed';
  if (kind === 'unstar') return view === 'starred';

  // Untagging is deliberately not here. The row only leaves if the tag removed
  // is the one being viewed, and this cannot see which tag was passed — so it
  // leaves the row alone rather than risk removing one the user is still
  // looking at. The next load has it right.
  return false;
}

export type UndoOffer = {
  actionId: number;
  description: string;
  /** The row exactly as it was, and where it sat. The engine restores captured
   *  prior state rather than inferring an inverse; the list does the same, so
   *  the two cannot drift apart. */
  row: Thread;
  atIndex: number;
  removed: boolean;
  /** Whether this was the conversation being read, so undo knows whether
   *  putting the row back should also put the selection back. */
  wasActive: boolean;
  /** The sidebar count nudge that went with it, taken back if the undo lands. */
  tagDelta?: { tagId: number; delta: number };
  /** The same, for the mailbox numbers. */
  viewDelta?: Record<string, number>;
  /** The other rows of a batch. One toast, one Z, all of them back: undo
   *  used to reach the last row only, and the rest stayed archived. */
  more?: UndoOffer[];
};

/**
 * Triage, applied the way the design describes it: the row leaves the list
 * immediately and the server catches up afterwards.
 *
 * The optimistic removal is the point. Archiving is the most repeated gesture in
 * a mail client, and waiting on a round trip before the row moves makes a
 * working client feel broken. The cost is that a failure has to put the row
 * back — and say so, rather than leaving a hole where a message used to be.
 */
/**
 * A row's tags after a tag or untag, or unchanged for anything else.
 *
 * Returns the same array when nothing applies, so React's identity check still
 * sees an unchanged row and does not re-render the whole list on every archive.
 */
function tagPatch(
  current: Thread['tags'],
  kind: ActionKind,
  targetId: number | undefined,
  tagById?: (id: number) => { name: string; colour: string } | undefined,
): Thread['tags'] {
  if (targetId == null) return current;
  if (kind === 'untag') {
    const tag = tagById?.(targetId);
    return tag ? current.filter((x) => x.name !== tag.name) : current;
  }
  if (kind !== 'tag') return current;
  const tag = tagById?.(targetId);
  if (!tag || current.some((x) => x.name === tag.name)) return current;
  return [...current, { id: targetId, name: tag.name, colour: tag.colour }];
}

/** The same nudge, pointing the other way — for a failure or an undo. */
function negated(deltas: Record<string, number>): Record<string, number> {
  return Object.fromEntries(Object.entries(deltas).map(([k, d]) => [k, -d]));
}

export function useTriage(opts: {
  items: Thread[];
  setItems: (fn: (prev: Thread[]) => Thread[]) => void;
  /** A tag by id, so a tagged row can show the tag before the server answers.
      The action carries an id; a row shows a name and a colour. */
  tagById?: (id: number) => { name: string; colour: string } | undefined;
  activeId: number | null;
  setActiveId: (id: number | null) => void;
  view: string;
  onMessage: (text: string, undo?: UndoOffer) => void;
  /** Moves a tag's count in the rail while the server catches up. */
  onTagCount?: (tagId: number, delta: number) => void;
  /** The same for the mailbox numbers, keyed by rail key. */
  onViewCount?: (deltas: Record<string, number>) => void;
  /** What the rail's numbers mean, so a nudge can agree with the recount. */
  /** What each sidebar mailbox counts, from the arrangement. */
  countModes?: Record<string, CountMode>;
  /** The role of a folder a `move` names, so the inbox's number can move. */
  folderRole?: (folderId: number) => string | undefined;
  /** Called once an action has settled either way, and after an undo — the
   *  moment the store can be asked for the real numbers. */
  onSettled?: () => void;
}) {
  const {
    items,
    setItems,
    activeId,
    setActiveId,
    view,
    onMessage,
    tagById,
    onTagCount,
    onViewCount,
    countModes = {},
    folderRole,
    onSettled,
  } = opts;
  const [pending, setPending] = useState(false);
  // The last thing done, so Z has something to reverse without the caller
  // tracking it.
  const lastUndo = useRef<UndoOffer | null>(null);
  // Conversations the user deliberately marked unread. Leaving one must not
  // mark it read again — flagging something to come back to is the entire
  // point, and undoing that on the way out is worse than never offering it.
  const heldUnread = useRef<Set<number>>(new Set());
  // While a batch runs, each row's undo offer is collected here instead of
  // becoming the toast, so the batch can offer them all at once.
  const collecting = useRef<UndoOffer[] | null>(null);

  const run = useCallback(
    async (kind: ActionKind, threadId?: number, targetId?: number, quiet = false) => {
      const k = kind;
      const target = threadId ?? activeId;
      if (target == null) {
        void api.log(JSON.stringify({ kind: 'triage', stage: 'no-target', k, activeId }));
        return;
      }
      const row = items.find((m) => m.id === target || m.thread_id === target);
      if (!row) {
        void api.log(
          JSON.stringify({ kind: 'triage', stage: 'no-row', k, target, rows: items.length }),
        );
        return;
      }
      void api.log(
        JSON.stringify({ kind: 'triage', stage: 'start', k, target, threadId: row.thread_id }),
      );

      // Deliberate only: the automatic mark-read is quiet, and it must not be
      // able to clear the flag a person set on purpose.
      if (kind === 'mark_unread' && !quiet) heldUnread.current.add(row.id);
      if (kind === 'mark_read') heldUnread.current.delete(row.id);

      const removes = leavesView(kind, view);
      const before = items;
      const atIndex = items.findIndex((m) => m.id === row.id);

      // The rail's tag number, moved with the row rather than a recount behind
      // it. Read off the same patch the row gets, so it counts only a tag the
      // conversation was not already wearing — tagging something twice is not
      // a second conversation. Undone below if the write fails, and again if
      // the action is undone; either way the debounced recount that follows
      // every triage is still the authority, so a nudge that gets this wrong
      // is wrong until that lands and no longer.
      const bump =
        targetId != null &&
        (kind === 'tag' || kind === 'untag') &&
        tagPatch(row.tags, kind, targetId, tagById) !== row.tags
          ? { tagId: targetId, delta: kind === 'tag' ? 1 : -1 }
          : null;
      if (bump) onTagCount?.(bump.tagId, bump.delta);

      // The mailbox numbers, moved on the same principle and reversed in the
      // same places. See countDeltas for the rule and for why it is allowed to
      // be approximate.
      const moved = countDeltas({
        kind,
        view,
        unread: row.unread,
        removes,
        modes: countModes,
        toRole: kind === 'move' && targetId != null ? folderRole?.(targetId) : undefined,
      });
      const movedAny = Object.keys(moved).length > 0;
      if (movedAny) onViewCount?.(moved);

      if (removes) {
        setItems((prev) => prev.filter((m) => m.id !== row.id));
        // Selection only follows when the row you archived is the one you were
        // reading — then it moves on, the way it would if you were working down
        // the list from the keyboard. Archiving some *other* row from its hover
        // button must leave the conversation you are reading where it is.
        if (row.id === activeId) {
          const next = items[atIndex + 1] ?? items[atIndex - 1] ?? null;
          setActiveId(next ? next.id : null);
        }
      } else {
        setItems((prev) =>
          prev.map((m) =>
            m.id === row.id
              ? {
                  ...m,
                  starred: kind === 'star' ? true : kind === 'unstar' ? false : m.starred,
                  unread:
                    kind === 'mark_unread' ? true : kind === 'mark_read' ? false : m.unread,
                  // Tags were missing from this patch, so a tag applied to a
                  // row stayed invisible until something else happened to
                  // reload the list — and the picker, which reads the row's
                  // tags back, showed the tick in the wrong state with it.
                  tags: tagPatch(m.tags, kind, targetId, tagById),
                }
              : m,
          ),
        );
      }

      setPending(true);
      try {
        const receipt = await api.triage(row.thread_id, kind, targetId);
        void api.log(JSON.stringify({ kind: 'triage', stage: 'ok', id: receipt.action_id }));
        // Quiet actions still queue for the server — the server has to learn
        // that you read it — but they announce nothing and offer no undo.
        // Reading is not a gesture you undo, and a toast for every conversation
        // you glance at would bury the ones that matter.
        if (quiet) return;
        const offer: UndoOffer = {
          actionId: receipt.action_id,
          description: receipt.description,
          row,
          atIndex,
          removed: removes,
          wasActive: row.id === activeId,
          tagDelta: bump ?? undefined,
          viewDelta: movedAny ? moved : undefined,
        };
        if (collecting.current) {
          collecting.current.push(offer);
        } else {
          lastUndo.current = offer;
          onMessage(receipt.description, offer);
        }
      } catch (err) {
        if (bump) onTagCount?.(bump.tagId, -bump.delta);
        if (movedAny) onViewCount?.(negated(moved));
        if (quiet) {
          // Nothing was optimistically removed for a quiet action, so there is
          // nothing to roll back; log it and leave the row as it was.
          void api.log(JSON.stringify({ kind: 'triage', stage: 'quiet-failed', k, error: String(err) }));
          return;
        }
        // A triage failure that only shows a toast is a failure nobody can
        // diagnose afterwards. It goes to the log as well.
        void api.log(JSON.stringify({ kind: 'triage', stage: 'failed', k, error: String(err) }));
        // Put it back. A row that vanished and did not actually move is worse
        // than one that never moved.
        setItems(() => before);
        setActiveId(row.id);
        onMessage(t('triage-failed', { error: String(err) }));
      } finally {
        setPending(false);
        onSettled?.();
      }
    },
    [
      items,
      activeId,
      setItems,
      setActiveId,
      view,
      onMessage,
      tagById,
      onTagCount,
      onViewCount,
      countModes,
      folderRole,
      onSettled,
    ],
  );

  /** The same action over several conversations, offered as one undo. */
  const runMany = useCallback(
    async (kind: ActionKind, ids: number[], targetId?: number) => {
      if (ids.length <= 1) {
        for (const id of ids) await run(kind, id, targetId);
        return;
      }
      const offers: UndoOffer[] = [];
      collecting.current = offers;
      try {
        for (const id of ids) await run(kind, id, targetId);
      } finally {
        collecting.current = null;
      }
      const [first, ...rest] = offers;
      if (!first) return;
      const offer: UndoOffer = { ...first, more: rest };
      lastUndo.current = offer;
      onMessage(
        t('triage-many', { what: first.description, count: String(offers.length) }),
        offer,
      );
    },
    [run, onMessage],
  );

  const undo = useCallback(
    async (offer?: UndoOffer) => {
      const target = offer ?? lastUndo.current;
      if (!target) return false;
      lastUndo.current = null;
      // Lowest index first, so each row lands where it sat and the ones
      // after it are still counted from the same start.
      const all = [target, ...(target.more ?? [])].sort((a, b) => a.atIndex - b.atIndex);
      let undone = 0;
      for (const one of all) {
        const ok = await api.undoTriage(one.actionId).catch(() => false);
        if (!ok) continue;
        undone += 1;
        // Put the row back where it was, rather than refetching the list. A
        // refetch would scroll you somewhere else and lose whatever you had
        // selected — which defeats the point of an undo you reach for in a
        // hurry. Splice by index, not by appending: the row belongs where it
        // was, and rows either side of it may have moved on since.
        setItems((prev) => {
          if (!one.removed) {
            return prev.map((m) => (m.id === one.row.id ? one.row : m));
          }
          if (prev.some((m) => m.id === one.row.id)) return prev;
          const at = Math.min(one.atIndex, prev.length);
          return [...prev.slice(0, at), one.row, ...prev.slice(at)];
        });
        if (one.removed && one.wasActive) setActiveId(one.row.id);
        if (one.tagDelta) onTagCount?.(one.tagDelta.tagId, -one.tagDelta.delta);
        if (one.viewDelta) onViewCount?.(negated(one.viewDelta));
      }
      const ok = undone === all.length;
      onMessage(ok ? t('undo-done') : t('undo-too-late'));
      onSettled?.();
      return ok;
    },
    [setItems, setActiveId, onMessage, onTagCount, onViewCount, onSettled],
  );

  /** S toggles, as it does everywhere else — one key, not two. */
  const toggleStar = useCallback(() => {
    const row = items.find((m) => m.id === activeId);
    if (!row) return;
    void run(row.starred ? 'unstar' : 'star');
  }, [items, activeId, run]);

  return {
    run,
    runMany,
    undo,
    toggleStar,
    pending,
    hasUndo: () => lastUndo.current != null,
    /** Whether the user asked for this conversation to stay unread. */
    isHeldUnread: (id: number) => heldUnread.current.has(id),
    /** Forgets that request, once the user opens the conversation again.
     *
     *  Marking something unread means "come back to this", not "never mark
     *  this read again" — so the hold lasts until you come back, and then
     *  opening it counts as coming back. Gmail, Outlook and Mail all behave
     *  this way, and without it a conversation marked unread once could never
     *  be marked read by reading it. */
    releaseHeldUnread: (id: number) => heldUnread.current.delete(id),
  };
}
