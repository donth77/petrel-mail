import { key } from './keys';

/** A shortcut row: what it does, and the keys that do it. */
export type Shortcut = { label: string; keys: string[] };
export type Group = { title: string; rows: Shortcut[] };

export function shortcutGroups(): Group[] {
  return [
    {
      title: 'Move around',
      rows: [
        { label: 'Next / previous conversation', keys: ['J', 'K'] },
        { label: 'Open conversation', keys: [key('enter')] },
        { label: 'Back to the list', keys: ['U'] },
        { label: 'Next / previous message in thread', keys: ['[', ']'] },
        { label: 'Cycle panes', keys: ['F6'] },
        { label: 'Go to Inbox · Starred · Sent · Drafts', keys: ['G', 'I S T D'] },
        { label: 'Switch active account', keys: [key('account')] },
      ],
    },
    {
      title: 'Write',
      rows: [
        { label: 'Compose', keys: ['C'] },
        { label: 'Reply · reply all · forward', keys: ['R', 'A', 'F'] },
        { label: 'Send', keys: [key('send')] },
        { label: 'Send later', keys: [key('sendLater')] },
        { label: 'Save draft', keys: [key('save')] },
        { label: 'Open in its own window', keys: [key('popout')] },
      ],
    },
    {
      title: 'Act on mail',
      rows: [
        { label: 'Archive', keys: ['E'] },
        { label: 'Move to trash', keys: ['#'] },
        { label: 'Report spam', keys: ['!'] },
        { label: 'Star', keys: ['S'] },
        { label: 'Snooze this conversation', keys: ['B'] },
        { label: 'Move to folder · tag', keys: ['V', 'L'] },
        { label: 'Mark read · unread', keys: [key('read'), key('unread')] },
        { label: 'Select · extend selection', keys: ['X', key('extend')] },
        { label: 'Undo the last thing', keys: ['Z'] },
      ],
    },
    {
      title: 'Everywhere',
      rows: [
        { label: 'Search', keys: ['/'] },
        { label: 'Command palette', keys: [key('palette')] },
        { label: 'This list', keys: ['?'] },
        { label: 'Settings', keys: [key('settings')] },
      ],
    },
  ];
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
