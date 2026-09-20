import { isKeyword, read, reading } from './search-grammar';

/**
 * The limits the engine holds a query to (`search_query.rs`).
 *
 * The engine knows when it stopped short, and keeps it to itself. The person
 * who typed the query is the one who needs to know, because a search that
 * quietly dropped their thirty-third term looks exactly like one that found
 * nothing for it. So the field reads the query the way the engine does and
 * counts. A test in the desktop crate holds these two numbers to the
 * engine's, so they cannot drift apart.
 */
export const MAX_CLAUSES = 32;
export const MAX_VALUE_CHARS = 256;

/** Why a query is not searched in full: more terms than the engine reads,
 *  or a term longer than it keeps. */
export type Cut = 'terms' | 'value';

/** Whether the engine will search less than the query says, and why. */
export function cutShort(query: string): Cut | null {
  let clauses = 0;
  let long = false;
  for (const l of read(query).lexemes) {
    if (l.kind !== 'piece' || isKeyword(l)) continue;
    const r = reading(l);
    // An empty pair of quotes asks nothing, and is not a term.
    if (r.key === null && r.value === '') continue;
    clauses += 1;
    if (clauses > MAX_CLAUSES) return 'terms';
    // A mailbox comes back lowered, as the engine holds it, and a few
    // letters lower to two: `İ` becomes `i̇`. So this is the length the
    // engine measures, whichever operator it was.
    long ||= chars(r.value) > MAX_VALUE_CHARS;
  }
  return long ? 'value' : null;
}

/** Characters as the engine counts them, not UTF-16 units: an emoji is one. */
const chars = (text: string) => [...text].length;
