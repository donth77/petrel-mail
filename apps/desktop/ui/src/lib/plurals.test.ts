import { describe, expect, it } from 'vitest';
import { setLocale, t } from './strings';

/* The six messages that ask Fluent to choose a plural form.
 *
 * Fluent picks the form by looking at the *number*. Every one of these was
 * called with an already-formatted string — `fmtCount(n)` or
 * `n.toLocaleString()` — and a string never matches `[one]`, so the plural
 * was chosen for every count: "1 more results", "All 1 messages were
 * searched". The number also still prints grouped, because Fluent formats it
 * with the bundle's own locale.
 *
 * Written against the ids rather than the call sites: the call sites pass
 * what these assert, and a future one that passes a string again will read
 * as wrong here.
 */
const PLURALS = [
  'palette-more',
  'empty-search-body',
  'accounts-storage',
  'storage-account-messages',
  'onb-progress-unknown',
  'delete-forever-many',
] as const;

describe('plural strings', () => {
  it('choose the singular for one and the plural for many', () => {
    setLocale('en');
    for (const id of PLURALS) {
      const one = t(id, { count: 1 });
      const many = t(id, { count: 5 });
      expect(one, `${id} rendered nothing for one`).toBeTruthy();
      expect(one, `${id} did not distinguish one from many`).not.toBe(many);
    }
  });

  it('a formatted string cannot choose the singular, which is the bug', () => {
    setLocale('en');
    for (const id of PLURALS) {
      // What the call sites used to pass. It renders the plural branch at
      // any count, so this is the shape that must never come back.
      expect(t(id, { count: '1' }), `${id} chose a form from a string`).toBe(
        t(id, { count: 5 }).replace('5', '1'),
      );
    }
  });

  it('still groups a large number, so nothing was lost by passing the number', () => {
    setLocale('en');
    expect(t('accounts-storage', { count: 12345 })).toContain('12,345');
  });
});
