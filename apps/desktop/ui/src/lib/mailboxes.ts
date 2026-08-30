import {
  Archive,
  Clock,
  Inbox,
  PencilLine,
  Send,
  ShieldAlert,
  Star,
  Trash2,
  Upload,
  type LucideIcon,
} from 'lucide-react';
import type { StringId } from './strings';

/**
 * How somebody has arranged the sidebar's mailboxes.
 *
 * Three things, and they are separate on purpose: the order the rows appear
 * in, which of them are shown at all, and what number each one carries. A
 * hidden mailbox is not a mailbox with its count turned off — unhiding it
 * should bring back the number it had.
 *
 * Only what differs from the defaults is stored. That way the defaults are
 * still live: if the engine's rule about what a mailbox counts is improved
 * later, it reaches everyone who never overrode that row, rather than being
 * frozen the first time somebody opened the settings pane.
 */

/** The wire words, shared with the engine's `CountMode`. The pane says None,
 *  Unread and All; these are what travel. */
export type CountMode = 'off' | 'unread' | 'total';

/** The fixed mailboxes, in the order they ship in. Mirrors `MAILBOX_KEYS`. */
export const MAILBOX_KEYS = [
  'inbox',
  'starred',
  'snoozed',
  'sent',
  'drafts',
  'outbox',
  'archive',
  'spam',
  'trash',
] as const;

export type MailboxKey = (typeof MAILBOX_KEYS)[number];

/**
 * What each mailbox is called and what it is drawn as.
 *
 * One map, because the rail and the Sidebar settings pane both draw this list
 * and a person reading the pane is matching it against the sidebar beside
 * them. Two copies would drift, and the first sign of it would be a settings
 * row wearing the wrong icon for the thing it turns off.
 */
export const MAILBOX_LOOK: Record<MailboxKey, { label: StringId; glyph: LucideIcon }> = {
  inbox: { label: 'mailbox-inbox', glyph: Inbox },
  starred: { label: 'mailbox-starred', glyph: Star },
  snoozed: { label: 'mailbox-snoozed', glyph: Clock },
  sent: { label: 'mailbox-sent', glyph: Send },
  drafts: { label: 'mailbox-drafts', glyph: PencilLine },
  outbox: { label: 'mailbox-outbox', glyph: Upload },
  archive: { label: 'mailbox-archive', glyph: Archive },
  spam: { label: 'mailbox-spam', glyph: ShieldAlert },
  trash: { label: 'mailbox-trash', glyph: Trash2 },
};

/** The one row nobody may hide. A mail client without an inbox is a puzzle. */
export const ESSENTIAL: MailboxKey = 'inbox';

/**
 * What a row counts when nobody has said otherwise. Mirrors the engine's
 * `default_count_mode`, and is the same one rule: a list you built by hand
 * counts everything on it, a place mail lands by itself counts what you have
 * not read, and nothing waits in Sent.
 */
export function defaultCount(key: string): CountMode {
  if (key === 'drafts' || key === 'outbox' || key === 'starred' || key === 'snoozed') return 'total';
  if (key === 'sent') return 'off';
  return 'unread';
}

export type Arrangement = {
  /** Every mailbox key, in the order they should be drawn. */
  order: MailboxKey[];
  /** Keys not drawn at all. Never contains the essential one. */
  hidden: MailboxKey[];
  /** Only the rows somebody changed. `folders` covers every folder they made. */
  counts: Record<string, CountMode>;
};

const MODES: CountMode[] = ['off', 'unread', 'total'];
const isKey = (k: unknown): k is MailboxKey =>
  typeof k === 'string' && (MAILBOX_KEYS as readonly string[]).includes(k);

/** The arrangement as it ships: shipping order, nothing hidden, no overrides. */
export function shipped(): Arrangement {
  return { order: [...MAILBOX_KEYS], hidden: [], counts: {} };
}

/**
 * Reads a stored arrangement, and is not fussy about it.
 *
 * Anything unreadable falls back to the shipped arrangement rather than
 * throwing: this is a sidebar preference, and a bad string in the settings
 * table must not be the reason somebody's mailboxes stop drawing.
 *
 * A key the list does not mention is appended in shipping order rather than
 * put at the front — the same rule folders already follow, so a mailbox added
 * in a later version turns up below the ones somebody arranged rather than
 * above them.
 */
export function parseArrangement(raw: string): Arrangement {
  const base = shipped();
  if (!raw.trim()) return base;
  let read: unknown;
  try {
    read = JSON.parse(raw);
  } catch {
    return base;
  }
  if (typeof read !== 'object' || read === null) return base;
  const from = read as Record<string, unknown>;

  const named = Array.isArray(from.order) ? from.order.filter(isKey) : [];
  const order = [...new Set(named), ...MAILBOX_KEYS.filter((k) => !named.includes(k))];

  const hidden = (Array.isArray(from.hidden) ? from.hidden.filter(isKey) : []).filter(
    (k) => k !== ESSENTIAL,
  );

  const counts: Record<string, CountMode> = {};
  if (typeof from.counts === 'object' && from.counts !== null) {
    for (const [key, value] of Object.entries(from.counts as Record<string, unknown>)) {
      if ((isKey(key) || key === 'folders') && MODES.includes(value as CountMode)) {
        counts[key] = value as CountMode;
      }
    }
  }
  return { order, hidden: [...new Set(hidden)], counts };
}

/** Writes one back, keeping only what differs from the defaults. */
export function serialiseArrangement(a: Arrangement): string {
  const counts: Record<string, CountMode> = {};
  for (const [key, mode] of Object.entries(a.counts)) {
    if (mode !== defaultCount(key)) counts[key] = mode;
  }
  return JSON.stringify({
    order: a.order,
    hidden: a.hidden.filter((k) => k !== ESSENTIAL),
    counts,
  });
}

/** The rows to draw, in order, with the essential one always present. */
export function visibleMailboxes(a: Arrangement): MailboxKey[] {
  return a.order.filter((k) => k === ESSENTIAL || !a.hidden.includes(k));
}

/** What this row counts, overridden or not. */
export function countFor(a: Arrangement, key: string): CountMode {
  return a.counts[key] ?? defaultCount(key);
}

/**
 * The map the counts query wants: every mailbox, plus folders.
 *
 * Hidden rows still ask for their number. The query is cheap, and the
 * alternative is a count that arrives a tick late every time somebody unhides
 * a mailbox — a row that appears empty and then fills in.
 */
export function countModes(a: Arrangement): Record<string, CountMode> {
  const out: Record<string, CountMode> = {};
  for (const key of MAILBOX_KEYS) out[key] = countFor(a, key);
  out.folders = countFor(a, 'folders');
  return out;
}

/**
 * The arrangement to use, given both settings.
 *
 * `badges` was the single switch this replaces: one choice of Unread, All or
 * None applied to every row, with a growing list of exceptions written into
 * its own help text. It is still read here, once, for anyone who had set it to
 * something other than the default — turning their counts off and then
 * silently turning them back on because the setting moved would be its own
 * small betrayal. Nothing writes it any more, and as soon as somebody arranges
 * their sidebar the stored arrangement wins.
 */
export function arrangementFor(stored: string, badges: string): Arrangement {
  if (stored.trim()) return parseArrangement(stored);
  const a = shipped();
  if (badges === 'off' || badges === 'total') {
    const mode: CountMode = badges === 'off' ? 'off' : 'total';
    for (const key of MAILBOX_KEYS) a.counts[key] = mode;
    a.counts.folders = mode;
  }
  return a;
}
