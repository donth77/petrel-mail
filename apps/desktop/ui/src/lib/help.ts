import { BINDINGS, displayKeys, GROUP_TITLES, type Binding } from './shortcuts';

/** A shortcut row: what it does, and the keys that do it. */
export type Shortcut = { label: string; keys: string[] };
export type Group = { title: string; rows: Shortcut[] };

export function shortcutGroups(): Group[] {
  // Rendered from the binding table, filtered to what is actually wired. A
  // shortcut cannot appear here without existing.
  const order: Binding['group'][] = ['move', 'write', 'act', 'everywhere'];
  return order
    .map((g) => ({
      title: GROUP_TITLES[g],
      rows: BINDINGS.filter((b) => b.group === g && b.available).map((b) => ({
        label: b.label,
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

export const OPERATOR_GROUPS: { title: string; ops: Operator[] }[] = [
  {
    title: 'Narrow by who and what',
    ops: [
      { op: 'from:', means: 'sender name or address', example: 'from:sam' },
      { op: 'to:', means: 'anyone in To' },
      { op: 'cc:', means: 'anyone copied' },
      { op: 'subject:', means: 'the subject line only, not the body' },
      { op: 'tag:', means: 'a tag you applied' },
    ],
  },
  {
    title: 'Narrow by where and how',
    ops: [
      { op: 'in:', means: 'inbox · archive · sent · drafts · spam · trash, or a folder name' },
      { op: 'is:', means: 'unread · read · starred · snoozed' },
      { op: 'has:attachment', means: 'carries a file' },
      { op: 'filename:', means: "an attached file's name", example: 'filename:.pdf' },
    ],
  },
  {
    title: 'Narrow by when',
    ops: [
      { op: 'after:', means: '', example: 'after:2026-06-01' },
      { op: 'before:', means: 'everything sent earlier' },
      { op: 'date:', means: 'one particular day' },
    ],
  },
  {
    title: 'Put terms together',
    ops: [
      { op: 'annex pricing', means: 'both words — several terms all have to match' },
      { op: '"board pack"', means: 'those words, in that order' },
      { op: '-draft', means: 'leave these out' },
      { op: 'OR', means: '', example: 'from:sam OR from:dana' },
    ],
  },
];
