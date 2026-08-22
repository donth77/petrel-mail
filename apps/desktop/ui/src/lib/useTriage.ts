import { useCallback, useRef, useState } from 'react';
import { api, type ActionKind, type Thread } from './api';
import { t } from './strings';

/** Whether an action takes a conversation out of the list you are looking at.
 *
 *  This cannot be read off the action alone. Archiving removes a row from the
 *  inbox and from trash, but not from the archive; unstarring removes it from
 *  Starred and from nowhere else. Deciding by action alone left rows sitting in
 *  lists they no longer belonged to. */
function leavesView(kind: ActionKind, view: string): boolean {
  // A move files the conversation somewhere specific, so it leaves whatever
  // list you were looking at — including the inbox, and including a folder
  // view, since the destination is by definition somewhere else.
  if (kind === 'move') return true;
  switch (view) {
    case 'inbox':
      return kind === 'archive' || kind === 'trash' || kind === 'spam';
    case 'starred':
      return kind === 'unstar' || kind === 'trash' || kind === 'spam';
    case 'archive':
      return kind === 'trash' || kind === 'spam';
    case 'trash':
      return kind === 'archive' || kind === 'spam';
    case 'spam':
      return kind === 'archive' || kind === 'trash';
    case 'sent':
    case 'drafts':
      return false;
    default:
      // Sent, drafts and tag views: nothing triage does moves a conversation
      // out of them.
      return false;
  }
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
export function useTriage(opts: {
  items: Thread[];
  setItems: (fn: (prev: Thread[]) => Thread[]) => void;
  activeId: number | null;
  setActiveId: (id: number | null) => void;
  view: string;
  onMessage: (text: string, undo?: UndoOffer) => void;
}) {
  const { items, setItems, activeId, setActiveId, view, onMessage } = opts;
  const [pending, setPending] = useState(false);
  // The last thing done, so Z has something to reverse without the caller
  // tracking it.
  const lastUndo = useRef<UndoOffer | null>(null);

  const run = useCallback(
    async (kind: ActionKind, threadId?: number, targetId?: number) => {
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

      const removes = leavesView(kind, view);
      const before = items;
      const atIndex = items.findIndex((m) => m.id === row.id);

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
                }
              : m,
          ),
        );
      }

      setPending(true);
      try {
        const receipt = await api.triage(row.thread_id, kind, targetId);
        void api.log(JSON.stringify({ kind: 'triage', stage: 'ok', id: receipt.action_id }));
        lastUndo.current = {
          actionId: receipt.action_id,
          description: receipt.description,
          row,
          atIndex,
          removed: removes,
          wasActive: row.id === activeId,
        };
        onMessage(receipt.description, lastUndo.current);
      } catch (err) {
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
      }
    },
    [items, activeId, setItems, setActiveId, view, onMessage],
  );

  const undo = useCallback(
    async (offer?: UndoOffer) => {
      const target = offer ?? lastUndo.current;
      if (!target) return false;
      const ok = await api.undoTriage(target.actionId).catch(() => false);
      lastUndo.current = null;
      if (ok) {
        // Put the row back where it was, rather than refetching the list. A
        // refetch would scroll you somewhere else and lose whatever you had
        // selected — which defeats the point of an undo you reach for in a
        // hurry. Splice by index, not by appending: the row belongs where it
        // was, and rows either side of it may have moved on since.
        setItems((prev) => {
          if (!target.removed) {
            return prev.map((m) => (m.id === target.row.id ? target.row : m));
          }
          if (prev.some((m) => m.id === target.row.id)) return prev;
          const at = Math.min(target.atIndex, prev.length);
          return [...prev.slice(0, at), target.row, ...prev.slice(at)];
        });
        if (target.removed && target.wasActive) setActiveId(target.row.id);
      }
      onMessage(ok ? t('undo-done') : t('undo-too-late'));
      return ok;
    },
    [setItems, setActiveId, onMessage],
  );

  /** S toggles, as it does everywhere else — one key, not two. */
  const toggleStar = useCallback(() => {
    const row = items.find((m) => m.id === activeId);
    if (!row) return;
    void run(row.starred ? 'unstar' : 'star');
  }, [items, activeId, run]);

  return { run, undo, toggleStar, pending, hasUndo: () => lastUndo.current != null };
}
