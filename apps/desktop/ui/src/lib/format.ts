/* Locale-aware formatting. Everything here follows the resolved locale — dates
   and number grouping are locale behaviour, not translation (docs 07 §13.5).
   The clock (12h vs 24h) comes from the locale too, so a user whose system is
   set to 24h sees 24h without configuring anything. */

export type ClockPref = 'system' | '12' | '24';

let locale = typeof navigator !== 'undefined' ? navigator.language || 'en' : 'en';
let clock: ClockPref = 'system';

/** Settings override; both default to whatever the OS already says. */
export function setFormatPrefs(next: { locale?: string; clock?: ClockPref }) {
  locale = next.locale ?? (typeof navigator !== 'undefined' ? navigator.language || 'en' : 'en');
  if (next.clock) clock = next.clock;
  rebuild();
}

let timeOnly: Intl.DateTimeFormat;
let weekday: Intl.DateTimeFormat;
let dayMonth: Intl.DateTimeFormat;
let withYear: Intl.DateTimeFormat;
let full: Intl.DateTimeFormat;

function rebuild() {
  const hour12 = clock === 'system' ? undefined : clock === '12';
  timeOnly = new Intl.DateTimeFormat(locale, { hour: 'numeric', minute: '2-digit', hour12 });
  weekday = new Intl.DateTimeFormat(locale, { weekday: 'short' });
  dayMonth = new Intl.DateTimeFormat(locale, { day: 'numeric', month: 'short' });
  withYear = new Intl.DateTimeFormat(locale, { day: 'numeric', month: 'short', year: 'numeric' });
  full = new Intl.DateTimeFormat(locale, { dateStyle: 'full', timeStyle: 'short', hour12 });
}
rebuild();

const DAY = 24 * 60 * 60 * 1000;

/**
 * The list column, in four tiers — each one answers "how long ago" with the
 * least information that still distinguishes it from its neighbours:
 *   today          → 14:02      (which part of today)
 *   last 7 days    → Tue        (which day this week)
 *   this year      → 20 Aug     (which day this year)
 *   older          → 20 Aug 2025
 */
export function listTime(ms: number, now = Date.now()): string {
  const d = new Date(ms);
  const n = new Date(now);
  if (d.toDateString() === n.toDateString()) return timeOnly.format(d);
  // Calendar days apart, not raw elapsed time: 23:50 yesterday is "yesterday",
  // not "today", even though it is under 24 hours ago.
  const midnight = new Date(n.getFullYear(), n.getMonth(), n.getDate()).getTime();
  if (ms > midnight - 6 * DAY) return weekday.format(d);
  if (d.getFullYear() === n.getFullYear()) return dayMonth.format(d);
  return withYear.format(d);
}

/**
 * The time on a message card: enough to place it, no more. The full date belongs
 * on hover and in the accessible name, not in the header, where it crowds out the
 * sender it sits beside.
 *   today          → Today 14:02
 *   yesterday      → Yesterday 09:14
 *   last 7 days    → Mon 09:14
 *   this year      → 20 Aug 14:02
 *   older          → 20 Aug 2025
 */
export function messageTime(ms: number, now = Date.now()): string {
  const d = new Date(ms);
  const n = new Date(now);
  const startOfToday = new Date(n.getFullYear(), n.getMonth(), n.getDate()).getTime();
  const clock = timeOnly.format(d);

  // "today"/"yesterday" come from the locale rather than being spelled here —
  // every other user-facing string does, and these are no different.
  const rel = new Intl.RelativeTimeFormat(locale, { numeric: 'auto' });
  const capitalise = (w: string) => w.charAt(0).toLocaleUpperCase(locale) + w.slice(1);

  if (ms >= startOfToday) return `${capitalise(rel.format(0, 'day'))} ${clock}`;
  if (ms >= startOfToday - DAY) return `${capitalise(rel.format(-1, 'day'))} ${clock}`;
  if (ms > startOfToday - 6 * DAY) return `${weekday.format(d)} ${clock}`;
  if (d.getFullYear() === n.getFullYear()) return `${dayMonth.format(d)} ${clock}`;
  return withYear.format(d);
}

/** Unabbreviated, for screen readers and tooltips — "Tue" reads as nothing. */
export function fullTime(ms: number): string {
  return full.format(new Date(ms));
}

export function count(n: number): string {
  return new Intl.NumberFormat(locale).format(n);
}

export function initials(display: string, addr: string): string {
  const source = display.trim() || addr;
  const parts = source.split(/[\s@._-]+/).filter(Boolean);
  if (parts.length === 0) return '?';
  if (parts.length === 1) return parts[0].slice(0, 2).toUpperCase();
  // An initial is a letter or a digit, never punctuation. "Pluto (YC)" splits
  // into ["Pluto", "(YC)"] and the second word's first character is a bracket,
  // so taking it blindly drew "P(" in the avatar. Anything that is not a
  // letter is dropped rather than substituted, which leaves "P" — the same
  // answer a person would give. Note it is the *first* character that is
  // tested, not the first letter found: reaching into "(YC)" for the Y would
  // turn a qualifier nobody thinks of as a name into half the initials.
  const first = [parts[0][0], parts[1][0]].filter((c) => /[\p{L}\p{N}]/u.test(c));
  return (first.length > 0 ? first.join('') : parts[0].slice(0, 2)).toUpperCase();
}

/** File sizes as people write them, not as computers store them. */
export function fileSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const kb = bytes / 1024;
  if (kb < 1024) return `${Math.round(kb)} KB`;
  return `${(kb / 1024).toFixed(1)} MB`;
}
