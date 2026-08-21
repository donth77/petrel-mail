/* Locale-aware formatting. Everything here follows the resolved locale — dates
   and number grouping are locale behaviour, not translation (docs 07 §13.5).
   The clock (12h vs 24h) comes from the locale too, so a user whose system is
   set to 24h sees 24h without configuring anything. */

export type ClockPref = 'system' | '12' | '24';

let locale = typeof navigator !== 'undefined' ? navigator.language || 'en' : 'en';
let clock: ClockPref = 'system';

/** Settings override; both default to whatever the OS already says. */
export function setFormatPrefs(next: { locale?: string; clock?: ClockPref }) {
  if (next.locale) locale = next.locale;
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
  return (parts[0][0] + parts[1][0]).toUpperCase();
}

/** File sizes as people write them, not as computers store them. */
export function fileSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const kb = bytes / 1024;
  if (kb < 1024) return `${Math.round(kb)} KB`;
  return `${(kb / 1024).toFixed(1)} MB`;
}
