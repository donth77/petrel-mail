import { BINDINGS, displayKeys, GROUP_TITLES, type Binding } from './shortcuts';
import { t, type StringId } from './strings';

/** A shortcut row: what it does, and the keys that do it. */
export type Shortcut = { label: string; keys: string[] };
export type Group = { title: string; rows: Shortcut[] };

export function shortcutGroups(): Group[] {
  // Rendered from the binding table, filtered to what is actually wired. A
  // shortcut cannot appear here without existing.
  const order: Binding['group'][] = ['move', 'write', 'act', 'everywhere'];
  return order
    .map((g) => ({
      title: t(GROUP_TITLES[g]),
      rows: BINDINGS.filter((b) => b.group === g && b.available).map((b) => ({
        label: t(b.label),
        keys: displayKeys(b),
      })),
    }))
    .filter((g) => g.rows.length > 0);
}

/**
 * The search operators, rendered from the canonical table in docs 07 §5.1.
 * A new operator is added there first and this follows, so the help and the
 * parser cannot drift apart.
 */
export type Operator = { op: string; means: string; example?: string };

/** The table as ids. Read through operatorGroups(), never directly: the
 *  strings have to be resolved before anything filters on them, or the Help
 *  dialog's search would be matching string ids instead of words. */
type OperatorIds = { op: string; means: StringId | ''; example?: string };

/** Which column a group sits in. The three that narrow a search stack on the
 *  left and the one about combining terms takes the right, so neither column
 *  is stretched to the height of a group beside it. */
type Side = 'left' | 'right';

const OPERATOR_IDS: { title: StringId; side: Side; ops: OperatorIds[] }[] = [
  {
    title: 'search-op-group-1',
    side: 'left',
    ops: [
      { op: 'from:', means: 'search-op-from', example: 'from:sam' },
      { op: 'to:', means: 'search-op-to' },
      { op: 'cc:', means: 'search-op-cc' },
      { op: 'subject:', means: 'search-op-subject' },
      { op: 'tag:', means: 'search-op-tag' },
    ],
  },
  {
    title: 'search-op-group-3',
    side: 'left',
    ops: [
      { op: 'after:', means: 'search-op-after', example: 'after:2026-06-01' },
      { op: 'before:', means: 'search-op-before' },
      { op: 'date:', means: 'search-op-date', example: 'date:2026-08-14' },
    ],
  },
  {
    title: 'search-op-group-2',
    side: 'left',
    ops: [
      { op: 'in:', means: 'search-op-in' },
      { op: 'is:', means: 'search-op-is' },
      { op: 'has:attachment', means: 'search-op-has-attachment' },
      { op: 'filename:', means: 'search-op-filename', example: 'filename:.pdf' },
    ],
  },
  {
    title: 'search-op-group-4',
    side: 'right',
    ops: [
      { op: 'annex pricing', means: 'search-op-annex-pricing' },
      { op: '"board pack"', means: 'search-op-board-pack' },
      // A placeholder, not a keyword: the row used to read `-draft`, which
      // made "draft" look like a word the app knows rather than any word you
      // put a minus in front of. No example either, because `-word` is the
      // example: "mail without this word: -draft" read as though `-draft`
      // were the answer rather than the same thing said twice.
      { op: '-word', means: 'search-op-draft' },
      { op: 'OR', means: 'search-op-or', example: 'from:sam OR from:dana' },
      { op: 'AND', means: 'search-op-and' },
      { op: 'NOT', means: 'search-op-not' },
      { op: '( )', means: 'search-op-brackets', example: '(from:sam OR from:dana) invoice' },
      // No row for the Gmail form, `from:(sam OR dana)`. The engine takes it
      // for every operator that holds a word or a state, so writing one of
      // them in the table made a general rule look like a special case for
      // senders, and a row per operator would be nine rows for a spelling.
      // It still parses; it is simply not taught here.
    ],
  },
];

/** The operator table in the language the window is speaking. */
export function operatorGroups(): { title: string; side: Side; ops: Operator[] }[] {
  return OPERATOR_IDS.map((g) => ({
    title: t(g.title),
    side: g.side,
    ops: g.ops.map((o) => ({ ...o, means: o.means ? t(o.means) : '' })),
  }));
}
