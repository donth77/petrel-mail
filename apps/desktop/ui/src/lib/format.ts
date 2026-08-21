/* Locale-aware formatting. Follows the resolved locale even while strings are
   English-only — dates and number grouping are locale behaviour, not translation
   (docs 07 §13.5). */

const locale = () => navigator.language || 'en';

const timeOnly = new Intl.DateTimeFormat(locale(), { hour: '2-digit', minute: '2-digit' });
const dayMonth = new Intl.DateTimeFormat(locale(), { day: 'numeric', month: 'short' });
const withYear = new Intl.DateTimeFormat(locale(), { day: 'numeric', month: 'short', year: 'numeric' });
const full = new Intl.DateTimeFormat(locale(), { dateStyle: 'full', timeStyle: 'short' });

/** List-column time: today shows the clock, this year the date, older the year. */
export function listTime(ms: number): string {
  const d = new Date(ms);
  const now = new Date();
  if (d.toDateString() === now.toDateString()) return timeOnly.format(d);
  if (d.getFullYear() === now.getFullYear()) return dayMonth.format(d);
  return withYear.format(d);
}

/** Unabbreviated, for screen readers and tooltips — "Tue 14:02" helps nobody read aloud. */
export function fullTime(ms: number): string {
  return full.format(new Date(ms));
}

export function count(n: number): string {
  return new Intl.NumberFormat(locale()).format(n);
}

export function initials(display: string, addr: string): string {
  const source = display.trim() || addr;
  const parts = source.split(/[\s@._-]+/).filter(Boolean);
  if (parts.length === 0) return '?';
  if (parts.length === 1) return parts[0].slice(0, 2).toUpperCase();
  return (parts[0][0] + parts[1][0]).toUpperCase();
}
