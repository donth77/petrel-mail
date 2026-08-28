/* Switching languages must repaint the window without emptying it.
 *
 * The provider used to wrap its children in `<Fragment key={resolved}>`. That
 * did repaint everything — by remounting the whole tree, which also threw away
 * every piece of React state below it. Choosing a language closed the Settings
 * window you chose it in, and lost the reader's scroll, the search box, and the
 * selection with it.
 *
 * Without the key, a re-render is enough: the provider hands out a new value
 * object, every useSettings() consumer re-renders, and App is one. The single
 * thing a re-render does not refresh is a useMemo holding translated text, so
 * those have to depend on the locale.
 *
 * There is no DOM renderer in this suite, so both halves are checked against
 * the source. That is not a lesser test here: the failure is a missing
 * dependency in an array, which is exactly what source can see and what a
 * passing render test would most likely miss anyway.
 */
import { describe, expect, it } from 'vitest';

/* Read through Vite rather than node:fs. The UI has no Node types, and the
   bundler already has every one of these files in hand. */
const SOURCES = import.meta.glob('../**/*.{ts,tsx}', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;

const sources = () =>
  Object.entries(SOURCES).filter(([path]) => !/\.test\.tsx?$/.test(path));

const settingsSource = () => {
  // Vite keys a file in this same directory as './settings.tsx', not
  // '../lib/settings.tsx'. Anchoring on the separator keeps this from also
  // matching components/settings/*.tsx.
  const found = Object.entries(SOURCES).find(([p]) => /(?:^|\/)settings\.tsx$/.test(p));
  if (!found) throw new Error('lib/settings.tsx not found by the glob');
  return found[1];
};

/** The hook call starting at `from`, up to its matching close paren. */
function callAt(src: string, from: number): string {
  let depth = 0;
  for (let i = from; i < src.length; i++) {
    if (src[i] === '(') depth++;
    else if (src[i] === ')' && --depth === 0) return src.slice(from, i + 1);
  }
  return src.slice(from);
}

describe('changing the language', () => {
  it('does not remount the tree to do it', () => {
    const provider = settingsSource();
    // Comments first, or this matches the note in settings.tsx explaining what
    // was removed — which is exactly what it did on the first run.
    const code = provider.replace(/\/\*[\s\S]*?\*\//g, '').replace(/^\s*\/\/.*$/gm, '');
    // A key on the children is the remount. Any key derived from the locale
    // brings back the bug this test exists for.
    expect(code).not.toMatch(/<[A-Za-z.]*\s+key=\{\s*resolved\s*\}/);
    expect(code).toMatch(/locale: resolved/);
  });

  it('keys every memoized translation on the locale', () => {
    /* A useMemo that calls t() and does not list `locale` keeps the words it
       computed in whatever language was current when it last ran. The Help
       dialog's shortcut table was the worst of these: deps of [], so it
       translated once per mount and never again. */
    const offenders: string[] = [];
    for (const [file, src] of sources()) {
      for (const m of src.matchAll(/use(?:Memo|Callback)\(/g)) {
        const call = callAt(src, m.index!);
        const translates = /\bt\(|shortcutGroups\(|operatorGroups\(|GROUP_TITLES/.test(call);
        if (!translates) continue;
        const deps = /,\s*(\[[^\]]*\])\s*\)$/.exec(call)?.[1];
        if (deps && !/\blocale\b/.test(deps)) {
          const line = src.slice(0, m.index).split('\n').length;
          offenders.push(`${file}:${line} deps=${deps}`);
        }
      }
    }
    expect(offenders).toEqual([]);
  });
});
