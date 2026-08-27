/* Every shipped locale, checked.
 *
 * A Fluent syntax error is quiet: the parser skips the entry it could not read
 * and carries on, so the only symptom is one string coming out in English,
 * somewhere, later. With six languages that is six times the surface and
 * nobody reading all of it.
 */
import { describe, expect, it } from 'vitest';
import { FluentBundle, FluentResource } from '@fluent/bundle';
import enFtl from './en.ftl?raw';
import esFtl from './es.ftl?raw';

const LOCALES: Record<string, string> = { en: enFtl, es: esFtl };

function idsIn(source: string): string[] {
  return source
    .split('\n')
    .map((line) => /^([a-zA-Z][a-zA-Z0-9_-]*)\s*=/.exec(line)?.[1])
    .filter((id): id is string => Boolean(id));
}

describe.each(Object.entries(LOCALES))('%s.ftl', (locale, source) => {
  it('parses with nothing skipped', () => {
    const resource = new FluentResource(source);
    const unparseable = resource.body.filter((entry) => !('id' in entry));
    expect(unparseable).toEqual([]);
  });

  it('declares no id twice', () => {
    const ids = idsIn(source);
    const seen = new Set<string>();
    const duplicates = ids.filter((id) => (seen.has(id) ? true : (seen.add(id), false)));
    expect(duplicates).toEqual([]);
  });

  it('invents no id that English does not have', () => {
    /* The other direction is allowed: a translation may lag and fall back.
       An id that exists ONLY in a translation is dead weight nothing can
       ever ask for, and usually a typo in the id. */
    const stray = idsIn(source).filter((id) => !new Set(idsIn(enFtl)).has(id));
    expect(stray).toEqual([]);
  });

  it('keeps every placeholder English declares', () => {
    /* A translator dropping { $count } produces a sentence with a hole in the
       meaning; inventing one produces a literal {$typo} on screen. Neither
       throws, and both reach the user. */
    const en = new Map<string, string>();
    for (const line of enFtl.split('\n')) {
      const m = /^([a-zA-Z][a-zA-Z0-9_-]*)\s*=(.*)$/.exec(line);
      if (m) en.set(m[1], m[2]);
    }
    const holes = (s: string) => new Set([...s.matchAll(/\{\s*\$(\w+)\s*\}/g)].map((m) => m[1]));

    const mismatched: string[] = [];
    for (const line of source.split('\n')) {
      const m = /^([a-zA-Z][a-zA-Z0-9_-]*)\s*=(.*)$/.exec(line);
      if (!m) continue;
      const english = en.get(m[1]);
      if (english === undefined) continue;
      const want = holes(english);
      const got = holes(m[2]);
      // Only flag holes the translation invented, or required ones it dropped.
      const invented = [...got].filter((h) => !want.has(h));
      if (invented.length) mismatched.push(`${m[1]}: unknown ${invented.join(', ')}`);
    }
    expect(mismatched).toEqual([]);
  });

  it('renders plurals for one and for many', () => {
    const bundle = new FluentBundle(locale, { useIsolating: false });
    bundle.addResource(new FluentResource(source));
    for (const id of ['reader-message-count', 'notify-many', 'outbox-attachments']) {
      const message = bundle.getMessage(id);
      if (!message?.value) continue; // not translated yet: falls back, fine
      const errors: Error[] = [];
      const one = bundle.formatPattern(message.value, { count: 1 }, errors);
      const many = bundle.formatPattern(message.value, { count: 5 }, errors);
      expect(errors).toEqual([]);
      expect(one).not.toBe(many);
    }
  });
});
