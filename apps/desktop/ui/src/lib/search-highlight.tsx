import { createContext, useContext } from 'react';
import { isKeyword, read, reading } from './search-grammar';

/**
 * Marking a search's words where they were found.
 *
 * The engine already marks the body text it quotes under a result. It cannot
 * mark the subject line, which is where a word is most often found, and it
 * has nothing to say about the message once it is open. So the words are
 * worked out here, from the query as typed, and marked wherever the result is
 * shown: the list's subjects, the reader's heading, the message itself.
 *
 * Only the words that were asked *for*. An excluded word is by definition
 * not in the result, a sender or a date is not text in the message, and
 * marking `sam` because the query said `from:sam` would light up every
 * signature he ever wrote.
 */
export type Term = {
  /** The word, or the words of a phrase, folded the way text is folded. */
  tokens: string[];
  /** The last word of a query is still being typed: `vend` finds "vendor",
   *  and the whole word is what gets marked. */
  prefix: boolean;
  /** CJK has no spaces to find a word's edges by, so a term is wherever its
   *  characters are, exactly as the per-character index matches it. */
  cjk: boolean;
};

const CJK = /[぀-ヿ㐀-䶿一-鿿豈-﫿가-힯]/;
const WORD = /[\p{L}\p{N}]/u;
/** An accent written as a character of its own, after the letter it sits on. */
const ACCENT = /\p{M}/u;

/** Lowercase and without its accents, one character for one character, so a
 *  position in the folded text is the same position in the original. FTS5
 *  folds the same way (`remove_diacritics`), which is why `cafe` finds
 *  "café" — and why the mark has to land on it too. */
export function fold(text: string): string {
  let out = '';
  for (const ch of text) {
    const bare = ch > '\x7f' ? ch.normalize('NFD').replace(/[̀-ͯ]/g, '') : ch;
    const low = bare.toLowerCase();
    out += low.length === ch.length ? low : ch;
  }
  return out;
}

/** The words a query asks for, in the order it asks for them. */
export function termsOf(query: string): Term[] {
  const { lexemes } = read(query);
  const terms: { text: string; typed: boolean }[] = [];
  // Whether each open bracket was excluded, and whether a NOT or a `-(` is
  // waiting for the thing it excludes.
  const excluded: boolean[] = [false];
  let pending = false;
  for (const l of lexemes) {
    const within = excluded[excluded.length - 1];
    if (l.kind === 'minus') pending = !pending;
    else if (l.kind === 'open') {
      excluded.push(within !== pending);
      pending = false;
    } else if (l.kind === 'close') {
      if (excluded.length > 1) excluded.pop();
      // A NOT with nothing after it but the bracket excludes nothing.
      pending = false;
    } else if (isKeyword(l)) {
      pending = l.says === 'NOT' ? !pending : false;
    } else {
      const r = reading(l);
      const out = within !== (pending !== r.negated);
      pending = false;
      if (out) continue;
      // As-you-type reaches into `subject:` too: `subject:invo` finds
      // "Invoice", and the subject is the one place it has to be marked.
      if (r.key === null) terms.push({ text: r.text, typed: l.plain });
      else if (r.key === 'subject') terms.push({ text: r.value, typed: l.plain });
    }
  }
  const shaped = terms.map(({ text, typed }) => {
    const folded = fold(text);
    // CJK is matched a run at a time, each wherever it falls: the index has
    // no phrases for it, so `"회의 일정"` is both runs and not the two joined.
    const cjk = CJK.test(folded);
    const tokens = cjk
      ? folded.split(/\s+/).filter(Boolean)
      : folded.split(/[^\p{L}\p{N}]+/u).filter(Boolean);
    return { typed, cjk, tokens };
  });
  // The word still being typed is the last one the index keeps, as in the
  // engine: in `lun -` the dash is the start of an exclusion, and `lun` is
  // still a prefix there.
  const last = shaped.map((term) => term.tokens.length > 0).lastIndexOf(true);
  return shaped.flatMap(({ typed, cjk, tokens }, i): Term[] => {
    if (tokens.length === 0) return [];
    if (cjk) return tokens.map((run) => ({ tokens: [run], prefix: false, cjk: true }));
    // The engine only completes a word of two letters or more. `vitamin c`
    // means the letter, not every word that starts with one.
    const growing = typed && i === last && [...tokens[tokens.length - 1]].length > 1;
    return [{ tokens, prefix: growing, cjk: false }];
  });
}

/**
 * Where the terms fall in a piece of text, as [start, end) pairs in order.
 *
 * A term matches the way the index matches it: whole words, in order, with
 * anything that is not a letter or a digit between them — `board pack` finds
 * "board-pack" — and never the inside of a longer word, so `art` does not
 * light up "party". No lookbehind: the app runs on WebKit builds that do not
 * have it, and a pattern that fails to compile there takes the list with it.
 *
 * `height_reporter.js` carries the same few lines for the message frame,
 * which nothing out here can reach into.
 */
export function hitsIn(text: string, terms: readonly Term[]): [number, number][] {
  if (terms.length === 0 || !text) return [];
  const low = fold(text);
  const word = (at: number) => at >= 0 && at < low.length && WORD.test(low[at]);
  const found: [number, number][] = [];
  for (const term of terms) {
    const first = term.tokens[0];
    for (let at = low.indexOf(first); at >= 0; at = low.indexOf(first, at + 1)) {
      if (!term.cjk && word(at - 1)) continue;
      let end = at + first.length;
      let whole = true;
      for (const next of term.tokens.slice(1)) {
        let gap = end;
        while (gap < low.length && !word(gap)) gap += 1;
        if (gap === end || !low.startsWith(next, gap)) {
          whole = false;
          break;
        }
        end = gap + next.length;
      }
      if (!whole) continue;
      if (!term.cjk) {
        // An accent typed as its own character belongs to the letter before
        // it: it is neither the end of the word nor outside the mark.
        const rest = (from: number) => {
          let to = from;
          while (to < low.length && ACCENT.test(low[to])) to += 1;
          return to;
        };
        end = rest(end);
        if (term.prefix) while (word(end)) end = rest(end + 1);
        else if (word(end)) continue;
      }
      found.push([at, end]);
    }
  }
  found.sort((a, b) => a[0] - b[0] || b[1] - a[1]);
  const merged: [number, number][] = [];
  for (const hit of found) {
    const before = merged[merged.length - 1];
    if (before && hit[0] <= before[1]) before[1] = Math.max(before[1], hit[1]);
    else merged.push(hit);
  }
  return merged;
}

/** Whether two lists of terms say the same thing. */
export function sameTerms(a: readonly Term[], b: readonly Term[]): boolean {
  return (
    a.length === b.length &&
    a.every(
      (term, i) =>
        term.prefix === b[i].prefix &&
        term.cjk === b[i].cjk &&
        term.tokens.length === b[i].tokens.length &&
        term.tokens.every((token, k) => token === b[i].tokens[k]),
    )
  );
}

/** No search, or highlighting switched off. One list, so it is never news
 *  to anything watching it. */
export const NO_TERMS: readonly Term[] = [];

/** The terms of the search on screen — empty when there is none, and when
 *  highlighting is switched off, so nothing downstream has to ask twice. */
export const SearchTerms = createContext<readonly Term[]>(NO_TERMS);

/** Text with the search's words marked: the search on screen, or the one
 *  given, for a window that has a query of its own (the palette). */
export function Marked({ text, terms }: { text: string; terms?: readonly Term[] }) {
  const onScreen = useContext(SearchTerms);
  const hits = hitsIn(text, terms ?? onScreen);
  if (hits.length === 0) return <>{text}</>;
  const out: React.ReactNode[] = [];
  let at = 0;
  hits.forEach(([start, end], i) => {
    if (start > at) out.push(text.slice(at, start));
    out.push(<mark key={i}>{text.slice(start, end)}</mark>);
    at = end;
  });
  if (at < text.length) out.push(text.slice(at));
  return <>{out}</>;
}
