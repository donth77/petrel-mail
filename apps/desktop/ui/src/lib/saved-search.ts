import type { SavedSearch } from './api';
import { isKeyword, read, reading } from './search-grammar';

/**
 * Saved searches, as the window has to reason about them: what to call a new
 * one, and whether the field still holds the one that is open.
 *
 * Both are here rather than in `App.tsx` because both are rules rather than
 * rendering, and a rule in a component is a rule nothing can test.
 */

/** The operators whose values are somebody's name or a place, and so keep a
 *  capital: `from:sam` is Sam. A word you searched for is not a name, and
 *  `is:unread` is not a proper noun. */
const NAMED = ['from', 'to', 'cc', 'in', 'tag', 'filename'];

/** At most this many values. A name is a label, not a restatement. */
const MOST = 5;

/**
 * A name to offer for the query in the field: the query with its syntax
 * stripped.
 *
 * `(from:sam OR from:dana) invoice` offers "Sam or Dana invoice";
 * `is:unread has:attachment` offers "Unread attachment"; `tag:waiting
 * -is:unread` offers "Waiting". Predictable, because it is what you typed with
 * the punctuation taken out — the first version offered one value from the
 * whole query, which named `(from:sam OR from:dana) invoice` "Sam" and left
 * three searches in a rail all called the same thing.
 *
 * What is left out is what a name cannot carry: an excluded term, because
 * "Waiting unread" would say the opposite of `-is:unread`, and the operators
 * themselves, because nobody needs a rail row called `tag:`.
 *
 * Empty when there is nothing to name from — pure punctuation, or operators
 * with no values yet — and the dialog then asks with a blank box.
 */
export function suggestName(query: string): string {
  const { lexemes } = read(query);
  const words: string[] = [];
  // `OR` between two values is worth keeping: it is the difference between
  // "Sam or Dana" and a list of two unrelated things.
  let alternative = false;
  for (const l of lexemes) {
    if (l.kind !== 'piece') continue;
    if (isKeyword(l)) {
      if (l.says === 'OR') alternative = true;
      continue;
    }
    const r = reading(l);
    // An excluded term cannot go in a name without its NOT, and a name with a
    // NOT in it is a query again.
    if (r.negated || l.nots.length % 2 === 1) continue;
    if (r.value === '' || words.length >= MOST) continue;
    const value = r.key !== null && NAMED.includes(r.key) ? capitalised(r.value) : r.value;
    words.push(alternative && words.length > 0 ? `or ${value}` : value);
    alternative = false;
  }
  return capitalised(words.join(' ').trim());
}

/** The first letter up, the rest as it was: `McBride` keeps its capital, and
 *  `élodie` becomes `Élodie` rather than being left alone as ASCII rules
 *  would. */
function capitalised(value: string): string {
  // Both edges: a leading `.pdf` names a filename search "Pdf", and a
  // half-typed `from:` names one "From" rather than "From:".
  const trimmed = value
    .trim()
    .replace(/^[^\p{L}\p{N}]+/u, '')
    .replace(/[^\p{L}\p{N})]+$/u, '');
  if (trimmed === '') return '';
  return trimmed.charAt(0).toLocaleUpperCase() + trimmed.slice(1);
}

/**
 * Where the field stands in relation to the saved searches.
 *
 * `none` — this is not a saved search, so it can be saved.
 * `saved` — it is one, unchanged, so there is nothing to do.
 * `edited` — it is one whose text has been changed, so it can be updated.
 */
export type SavedState = { kind: 'none' } | { kind: 'saved' | 'edited'; search: SavedSearch };

export function savedState(view: string, query: string, searches: SavedSearch[]): SavedState {
  const search = view.startsWith('search:')
    ? searches.find((s) => `search:${s.id}` === view)
    : undefined;
  if (!search) return { kind: 'none' };
  // Compared as typed, because that is how it is stored and how it goes back
  // into the field. Trimmed at the edges only: trailing space while somebody
  // types the next word is not an edit worth offering to save.
  return search.query.trim() === query.trim()
    ? { kind: 'saved', search }
    : { kind: 'edited', search };
}

/**
 * Whether what is in the field can be saved.
 *
 * True for any search that is not already pinned — including one reached by
 * editing a saved search, because editing a saved search's text means you are
 * running a different search, not amending that one. There is deliberately no
 * "update this saved search": a saved search is a question somebody wrote down,
 * and the way to change what it asks is to write down the new one. Offering
 * both read as an invitation to overwrite the thing you had just opened.
 *
 * False for the mailbox, and false for a saved search exactly as saved — there
 * is nothing to save that is not saved.
 */
export function canSave(searching: boolean, state: SavedState): boolean {
  return searching && state.kind !== 'saved';
}
