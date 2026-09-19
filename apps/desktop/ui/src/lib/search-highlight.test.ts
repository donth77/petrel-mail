import { describe, expect, it } from 'vitest';
import { fold, hitsIn, termsOf, type Term } from './search-highlight';

/** The words a query marks, as plain strings, with `*` on the one still
 *  being typed. */
const words = (query: string) =>
  termsOf(query).map((term) => term.tokens.join(' ') + (term.prefix ? '*' : ''));

/** The text with its hits in brackets, so an assertion reads at a glance. */
function marked(text: string, query: string): string {
  let out = '';
  let at = 0;
  for (const [start, end] of hitsIn(text, termsOf(query))) {
    out += `${text.slice(at, start)}[${text.slice(start, end)}]`;
    at = end;
  }
  return out + text.slice(at);
}

describe('the words a query asks for', () => {
  it('are its words, not its conditions', () => {
    expect(words('(from:sam OR from:dana) vendor')).toEqual(['vendor*']);
    expect(words('in:inbox has:attachment is:unread after:2026 annex')).toEqual(['annex*']);
    expect(words('to:dana cc:legal tag:urgent filename:.pdf')).toEqual([]);
  });

  it('include a phrase whole, and the words of subject:', () => {
    expect(words('"board pack" annex')).toEqual(['board pack', 'annex*']);
    expect(words('subject:invoice')).toEqual(['invoice']);
    expect(words('subject:"board pack" annex')).toEqual(['board pack', 'annex*']);
  });

  /* `contract` is still the last word asked *for*, so it is still the one
     being typed — which is how the engine reads it too, and why `contract
     -draft` finds "contracts". */
  it('never include what was excluded, however it was excluded', () => {
    expect(words('contract -draft')).toEqual(['contract*']);
    expect(words('contract NOT draft')).toEqual(['contract*']);
    expect(words('contract -"board pack"')).toEqual(['contract*']);
    expect(words('contract -(draft OR wip)')).toEqual(['contract*']);
    expect(words('contract NOT (draft OR wip) signed')).toEqual(['contract', 'signed*']);
    expect(words('contract -subject:draft')).toEqual(['contract*']);
    // Excluded twice is asked for.
    expect(words('NOT -draft')).toEqual(['draft*']);
    expect(words('-(contract -(draft))')).toEqual(['draft*']);
  });

  it('leave the operators themselves out', () => {
    expect(words('invoice AND receipt OR statement')).toEqual(['invoice', 'receipt', 'statement*']);
    expect(words('now or never')).toEqual(['now', 'or', 'never*']);
    expect(words('"OR" theatre')).toEqual(['or', 'theatre*']);
  });

  it('treat only the last word, typed bare, as still being typed', () => {
    expect(words('annex pricing')).toEqual(['annex', 'pricing*']);
    expect(words('annex "pricing"')).toEqual(['annex', 'pricing']);
    expect(words('ann has:attachment')).toEqual(['ann*']);
  });

  it('read an unknown operator as the words it is', () => {
    expect(words('re:pricing')).toEqual(['re pricing*']);
  });

  it('keep CJK as the run of characters it is', () => {
    expect(termsOf('東京 会議')).toEqual<Term[]>([
      { tokens: ['東京'], prefix: false, cjk: true },
      { tokens: ['会議'], prefix: true, cjk: true },
    ]);
  });
});

describe('where they fall in a piece of text', () => {
  it('whatever the case', () => {
    expect(marked('Q3 Vendor contracts', '(from:sam OR from:dana) vendor')).toBe(
      'Q3 [Vendor] contracts',
    );
    expect(marked('VENDOR SHORTLIST', 'vendor shortlist')).toBe('[VENDOR] [SHORTLIST]');
  });

  it('whole words only, the way the index matches them', () => {
    expect(marked('The party started the art class', '"art" class')).toBe(
      'The party started the [art] [class]',
    );
    // Not the last word, so not a prefix: "vendors" is a different word.
    expect(marked('vendor and vendors', 'vendor list')).toBe('[vendor] and vendors');
  });

  it('the whole word when the last one is still being typed', () => {
    expect(marked('Q3 vendor contracts', 'vend')).toBe('Q3 [vendor] contracts');
    expect(marked('Q3 vendor contracts', 'contract')).toBe('Q3 vendor [contracts]');
  });

  it('a phrase across whatever is between its words, and only in order', () => {
    expect(marked('Board pack v4', '"board pack"')).toBe('[Board pack] v4');
    expect(marked('the board-pack, final', '"board pack"')).toBe('the [board-pack], final');
    expect(marked('pack the board', '"board pack"')).toBe('pack the board');
    expect(marked('boardpack', '"board pack"')).toBe('boardpack');
  });

  it('through accents, as the index does', () => {
    expect(marked('Rendez-vous au Café', 'cafe rendez')).toBe('[Rendez]-vous au [Café]');
    expect(marked('Élodie Martin', 'élodie')).toBe('[Élodie] Martin');
    expect(fold('Café ÉLODIE')).toBe('cafe elodie');
  });

  it('CJK wherever its characters are', () => {
    expect(marked('東京支社の会議について', '会議')).toBe('東京支社の[会議]について');
    expect(marked('東京都', '東京')).toBe('[東京]都');
  });

  it('as one mark where two terms overlap', () => {
    expect(marked('board pack', '"board pack" board')).toBe('[board pack]');
  });

  it('nothing at all when nothing was asked for', () => {
    expect(hitsIn('Q3 vendor contracts', [])).toEqual([]);
    expect(hitsIn('', termsOf('vendor'))).toEqual([]);
    expect(marked('from sam, with love', 'from:sam')).toBe('from sam, with love');
  });

  it('keeps its place in text whose folding could change its length', () => {
    // İ lowercases to two code units; the fold must not shift what follows.
    expect(marked('İstanbul vendor', 'vendor')).toBe('İstanbul [vendor]');
    expect(marked('😀 vendor 😀', 'vendor')).toBe('😀 [vendor] 😀');
  });
});
