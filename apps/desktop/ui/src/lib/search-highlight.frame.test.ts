import { describe, expect, it } from 'vitest';
import frame from '../../../src-tauri/src/height_reporter.js?raw';
import { fold, hitsIn, termsOf, type Term } from './search-highlight';

/**
 * The message frame has its own copy of the matcher, and it has to agree.
 *
 * Nothing outside the frame can read the document inside it, so the app
 * cannot mark the message's text from here: the words are sent in and the
 * frame marks its own (`height_reporter.js`). That leaves two copies of the
 * same few lines, and two copies drift. The frame's are lifted out of its
 * source here and run against the same text as ours.
 *
 * Only the matching is checked, because only the matching is duplicated. The
 * walking and marking of the document around it needs a document, and is
 * proven in the harness instead.
 */
const chunk = (() => {
  const from = frame.indexOf('var searchTerms = [];');
  const to = frame.indexOf('function clearSearch()');
  if (from < 0 || to < 0 || to < from) {
    throw new Error('height_reporter.js no longer holds its matcher where this test looks');
  }
  return frame.slice(from, to);
})();

/** The frame's `hitsIn(fold(text))`, with the terms as they are sent in. */
const inTheFrame = new Function(
  `${chunk}\nreturn function (text, sent) { searchTerms = sent; return hitsIn(fold(text)); };`,
)() as (text: string, sent: { t: string[]; p: boolean; c: boolean }[]) => [number, number][];

/** What `MessageBody` posts in. */
const sent = (terms: readonly Term[]) =>
  terms.map((term) => ({ t: [...term.tokens], p: term.prefix, c: term.cjk }));

/** The text with its hits in brackets, from whichever copy is asked. */
function marked(text: string, terms: readonly Term[], hits: [number, number][]): string {
  let out = '';
  let at = 0;
  for (const [start, end] of hits) {
    out += `${text.slice(at, start)}[${text.slice(start, end)}]`;
    at = end;
  }
  return out + text.slice(at);
}

describe('the frame marks what the app marks', () => {
  const cases: [string, string][] = [
    ['Q3 vendor contracts', 'vendor'],
    ['Q3 Vendor contracts', 'vendor'],
    ['The board pack is attached', '"board pack"'],
    ['The board-pack is attached', '"board pack"'],
    ['We pack the board once', '"board pack"'],
    ['Vendor contracts', 'vend'],
    ['Vendor contracts', '"vend"'],
    ['A party for art', 'art'],
    ['Vitamin C and calcium', 'vitamin c'],
    ['Café con leche', 'cafe'],
    ['CAFÉ CON LECHE', 'cafe'],
    ['café au lait', 'cafe'],
    // A word still being typed, running on over an accent of its own.
    ['café au lait', 'caf'],
    ['cafés au lait', 'caf'],
    ['İstanbul to Ankara', 'istanbul'],
    ['東京の会議です', '東京'],
    ['회의 일정 안내', '"회의 일정"'],
    ['Re: invoice 2214 (paid)', 'invoice 2214'],
    ['no match here', 'absent'],
    ['annex annex annex', 'annex'],
    ['subject line', 'subject:subject'],
    ['a b c', 'a'],
    ['Lunch on Friday', 'lun -'],
  ];

  it.each(cases)('%s — %s', (text, query) => {
    const terms = termsOf(query);
    const ours = hitsIn(text, terms);
    expect(marked(text, terms, inTheFrame(text, sent(terms)))).toBe(marked(text, terms, ours));
  });

  it('folds the same characters', () => {
    const inFrameFold = new Function(
      `${chunk}\nreturn fold;`,
    )() as (text: string) => string;
    for (const text of ['Café', 'İstanbul', 'STRASSE', 'ﬁnd', '東京', 'ǅungla', '😀 a', 'ß']) {
      expect(inFrameFold(text)).toBe(fold(text));
      expect(inFrameFold(text).length).toBe(text.length);
    }
  });
});
