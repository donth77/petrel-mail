import type { Thread, ThreadIndexRow } from './api';

/** Estimated height of a collapsed reader card, including the gap below it. */
export const COLLAPSED_ROW = 52;

/** Estimated height of an expanded card before the body reports its size. */
export const EXPANDED_ROW_ESTIMATE = 320;

/** Messages asked for in one thread_detail call.
 *
 *  The reading pane no longer pages this way. Reply/forward still ask for
 *  the newest message with limit 1; the IPC cap stays for that fat path. */
export const THREAD_PAGE = 50;

/** Engine and IPC both cap a thread page here. */
export const THREAD_PAGE_MAX = 100;

/** How many message bodies may mount at once. Each body is a sandboxed frame;
 *  uncapped expansion turned a long thread into gigabytes of WebView. */
export const MAX_OPEN_BODIES = 3;

/** Keep drawing the current cards while the same conversation reloads.
 *
 *  A first open, or a switch to another thread, paints the listing's newest
 *  message at once. Reloading the thread you are already in must not blank
 *  the pane. */
export function keepExistingPane(args: {
  loadedThreadId: number | null;
  requestedThreadId: number;
}): boolean {
  return args.loadedThreadId === args.requestedThreadId;
}

/** Slim card from the list row. The pane mounts this before `thread_index`
 *  returns, so the newest body does not wait on the rest of the conversation. */
export function previewCard(thread: Thread): ThreadIndexRow {
  return {
    id: thread.id,
    from_display: thread.from_display,
    from_addr: thread.from_addr,
    snippet: thread.snippet,
    date_ms: thread.date_ms,
    unread: thread.unread,
  };
}

/** Cards that sit above the pinned newest body.
 *
 *  The index is oldest-first and includes the message already on screen.
 *  Those older rows are the conversation; the newest stays outside the
 *  list so its frame is not rebuilt when they arrive. A one-message
 *  conversation has nothing to put here. */
export function olderCards(args: {
  index: readonly ThreadIndexRow[];
  newestId: number;
}): ThreadIndexRow[] {
  return args.index.filter((row) => row.id !== args.newestId);
}

/** Clamp a requested page size to what thread_detail accepts. */
export function clampThreadLimit(limit: number): number {
  if (!Number.isFinite(limit)) return THREAD_PAGE;
  return Math.min(THREAD_PAGE_MAX, Math.max(1, Math.floor(limit)));
}

/** Which messages stay expanded after opening another.
 *
 *  Always keeps the one just opened and, when it is not the same row, the
 *  newest — those are what you came for and what you are reading toward.
 *  Past that, insertion order decides what drops: the oldest expand first. */
export function nextExpanded(args: {
  prev: ReadonlySet<number>;
  add: number;
  newestId: number | null;
}): Set<number> {
  const next = new Set(args.prev);
  next.add(args.add);

  while (next.size > MAX_OPEN_BODIES) {
    let dropped = false;
    for (const id of next) {
      if (id === args.add || id === args.newestId) continue;
      next.delete(id);
      dropped = true;
      break;
    }
    if (!dropped) break;
  }
  return next;
}

/** Which expanded messages may mount a body frame.
 *
 *  A header can stay open without an iframe underneath. Newest wins a slot when
 *  it is expanded; the rest go to the most recently opened. */
export function bodiesToMount(
  expanded: ReadonlySet<number>,
  newestId: number | null,
): Set<number> {
  if (expanded.size === 0) return new Set();

  const ordered = [...expanded];
  const out = new Set<number>();

  if (newestId != null && expanded.has(newestId)) {
    out.add(newestId);
  }

  for (let i = ordered.length - 1; i >= 0 && out.size < MAX_OPEN_BODIES; i -= 1) {
    const id = ordered[i];
    if (id !== newestId) out.add(id);
  }

  return out;
}
