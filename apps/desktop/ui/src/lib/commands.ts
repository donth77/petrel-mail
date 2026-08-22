import type { LucideIcon } from 'lucide-react';
import {
  Archive, ArrowRight, BellOff, Clock, Inbox, PencilLine, Reply, Search, Settings,
  Star, Tag, Trash2, CircleHelp, Send, FolderInput,
} from 'lucide-react';
import type { ActionKind } from './api';
import { t, type StringId } from './strings';

/**
 * Commands are grouped by *what they act on*, and the group states it — the
 * conversation group carries the subject, so "Snooze" can only be read one way
 * (docs 06). App-wide commands live in their own group and never mix in.
 */
export type CommandScope = 'conversation' | 'goto' | 'app';

export type Command = {
  id: string;
  scope: CommandScope;
  label: StringId;
  /** The product's own word for something the user knows by another name. */
  alias?: StringId;
  /** What kind of thing this is — "Archive" the folder vs "Archive" the action
   *  both appear in one list, and the group label alone leaves it ambiguous. */
  hint?: StringId;
  icon: LucideIcon;
  keys?: string[];
  run: () => void;
};

export type CommandContext = {
  hasThread: boolean;
  onView: (view: string) => void;
  /** The same action the keyboard runs. The palette is a way to *find* a
   *  command, not a second implementation of it — routing it anywhere else is
   *  how the two drift until one of them is quietly wrong. */
  onAction: (kind: ActionKind) => void;
  onSnooze: () => void;
  onMove: () => void;
  onTag: () => void;
  onCompose: () => void;
  onReply: () => void;
  onPauseNotifications: () => void;
  onNotImplemented: (label: string) => void;
};

export function buildCommands(ctx: CommandContext): Command[] {
  const todo = (label: string) => () => ctx.onNotImplemented(label);

  const conversation: Command[] = [
    { id: 'archive', scope: 'conversation', label: 'cmd-archive', alias: 'cmd-archive-alias', icon: Archive, keys: ['E'], run: () => ctx.onAction('archive') },
    { id: 'snooze', scope: 'conversation', label: 'cmd-snooze', icon: Clock, keys: ['B'], run: ctx.onSnooze },
    { id: 'star', scope: 'conversation', label: 'cmd-star', icon: Star, keys: ['S'], run: () => ctx.onAction('star') },
    { id: 'tag', scope: 'conversation', label: 'cmd-tag', icon: Tag, keys: ['L'], run: ctx.onTag },
    { id: 'move', scope: 'conversation', label: 'cmd-move', icon: FolderInput, keys: ['V'], run: ctx.onMove },
    { id: 'reply', scope: 'conversation', label: 'cmd-reply', icon: Reply, keys: ['R'], run: ctx.onReply },
    { id: 'trash', scope: 'conversation', label: 'cmd-trash', icon: Trash2, keys: ['#'], run: () => ctx.onAction('trash') },
  ];

  const goto: Command[] = [
    { id: 'go-inbox', scope: 'goto', label: 'mailbox-inbox', hint: 'hint-folder', icon: Inbox, keys: ['G', 'I'], run: () => ctx.onView('inbox') },
    { id: 'go-starred', scope: 'goto', label: 'mailbox-starred', hint: 'hint-folder', icon: Star, keys: ['G', 'S'], run: () => ctx.onView('starred') },
    { id: 'go-snoozed', scope: 'goto', label: 'mailbox-snoozed', hint: 'hint-folder', icon: Clock, run: () => ctx.onView('snoozed') },
    { id: 'go-sent', scope: 'goto', label: 'mailbox-sent', hint: 'hint-folder', icon: Send, keys: ['G', 'T'], run: () => ctx.onView('sent') },
    { id: 'go-drafts', scope: 'goto', label: 'mailbox-drafts', hint: 'hint-folder', icon: PencilLine, keys: ['G', 'D'], run: () => ctx.onView('drafts') },
    { id: 'go-archive', scope: 'goto', label: 'mailbox-archive', hint: 'hint-folder', icon: Archive, run: () => ctx.onView('archive') },
    { id: 'go-trash', scope: 'goto', label: 'mailbox-trash', hint: 'hint-folder', icon: Trash2, run: () => ctx.onView('trash') },
  ];

  const app: Command[] = [
    { id: 'compose', scope: 'app', label: 'cmd-compose', icon: PencilLine, keys: ['C'], run: ctx.onCompose },
    { id: 'search', scope: 'app', label: 'cmd-search', icon: Search, keys: ['/'], run: () => ctx.onView('search') },
    { id: 'pause', scope: 'app', label: 'cmd-pause-notifications', icon: BellOff, run: ctx.onPauseNotifications },
    { id: 'help', scope: 'app', label: 'rail-help', icon: CircleHelp, keys: ['?'], run: () => ctx.onView('help') },
    { id: 'settings', scope: 'app', label: 'rail-settings', icon: Settings, run: () => ctx.onView('settings') },
  ];

  // Conversation actions are meaningless with nothing selected, so they are
  // absent rather than present-and-inert: an unrunnable command in a palette
  // teaches people the palette is unreliable.
  return [...(ctx.hasThread ? conversation : []), ...goto, ...app];
}

/**
 * Match a query against a label, preferring the best match rather than the
 * first one found.
 *
 * A plain greedy subsequence scan finds "arch" in "M(a)(r)k Done (Ar[ch]ive)" —
 * technically a match, and a much worse one than the contiguous "Arch" sitting
 * right there. So contiguous substrings are tried first, and subsequence is the
 * fallback for initialisms like "gi" → "Go to Inbox".
 */
export function fuzzyMatch(query: string, text: string): number[] | null {
  if (!query) return [];
  const q = query.toLowerCase();
  const s = text.toLowerCase();

  const at = s.indexOf(q);
  if (at >= 0) return Array.from({ length: q.length }, (_, i) => at + i);

  const hits: number[] = [];
  let qi = 0;
  for (let i = 0; i < s.length && qi < q.length; i++) {
    if (s[i] === q[qi]) {
      hits.push(i);
      qi++;
    }
  }
  return qi === q.length ? hits : null;
}

/**
 * Higher is better. A match at the start of a word beats one buried mid-word
 * ("arch" in "Archive" over "Search"), contiguity beats scatter, and an earlier
 * position breaks the tie.
 */
export function scoreMatch(hits: number[], text: string): number {
  if (hits.length === 0) return 0;
  const start = hits[0];
  const atWordStart = start === 0 || /[\s(\-/]/.test(text[start - 1] ?? '');
  let contiguous = 0;
  for (let i = 1; i < hits.length; i++) if (hits[i] === hits[i - 1] + 1) contiguous++;
  return (atWordStart ? 40 : 0) + contiguous * 4 - start;
}

/** The plain name, before any alias or hint. */
export function nameOf(c: Command): string {
  return t(c.label);
}

/** The muted trailing part: an alias in parentheses, or a type hint after a dash. */
export function suffixOf(c: Command): string {
  if (c.alias) return ` (${t(c.alias)})`;
  if (c.hint) return ` — ${t(c.hint)}`;
  return '';
}

/** What matching runs against: everything the user can see on the row. */
export function labelOf(c: Command): string {
  return nameOf(c) + suffixOf(c);
}
