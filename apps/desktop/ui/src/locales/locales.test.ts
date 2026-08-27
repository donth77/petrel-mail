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
import frFtl from './fr.ftl?raw';
import deFtl from './de.ftl?raw';
import ptBrFtl from './pt-BR.ftl?raw';
import jaFtl from './ja.ftl?raw';

const LOCALES: Record<string, string> = { en: enFtl, es: esFtl, fr: frFtl, de: deFtl, 'pt-BR': ptBrFtl, ja: jaFtl };

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
       throws, and both reach the user.

       Entries are read whole, not line by line. A plural is a multi-line
       select expression whose first line is `{ $count ->`, which is not a
       placeable and does not match the placeholder pattern. Reading one line
       at a time therefore concluded that English declared no $count at all,
       and flagged every correct translation of those strings as inventing
       one. The test was wrong, not the translations. */
    const holes = (value: string) =>
      new Set([...value.matchAll(/\{\s*\$(\w+)\s*(?:\}|->)/g)].map((m) => m[1]));

    const entries = (src: string) => {
      const out = new Map<string, string>();
      let id: string | null = null;
      let buf: string[] = [];
      const flush = () => {
        if (id) out.set(id, buf.join('\n'));
        id = null;
        buf = [];
      };
      for (const line of src.split('\n')) {
        const m = /^([a-zA-Z][a-zA-Z0-9_-]*)\s*=(.*)$/.exec(line);
        if (m) {
          flush();
          id = m[1];
          buf = [m[2]];
        } else if (id && (line.startsWith(' ') || line.trim() === '}')) {
          buf.push(line);
        } else if (!line.trim() || line.startsWith('#')) {
          flush();
        }
      }
      flush();
      return out;
    };

    const en = entries(enFtl);
    const mismatched: string[] = [];
    for (const [id, value] of entries(source)) {
      const english = en.get(id);
      if (english === undefined) continue;
      const want = holes(english);
      const invented = [...holes(value)].filter((h) => !want.has(h));
      if (invented.length) mismatched.push(`${id}: unknown ${invented.join(', ')}`);
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
