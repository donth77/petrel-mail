/**
 * The keyboard map, in one place.
 *
 * Scattering key handling across components is how a shortcut ends up working
 * in one pane and not another, so every binding is declared here and the Help
 * sheet renders from the same list — a shortcut cannot be documented without
 * existing, or exist without being documented.
 *
 * `available: false` means designed but not built. Those are hidden from Help
 * rather than shown greyed: a sheet that lists dead keys teaches keystrokes
 * that fail, which is worse than a shorter sheet.
 */
import { key, type KeyName } from './keys';

export type Chord = { key: string; shift?: boolean; meta?: boolean; then?: string };

export type Binding = {
  id: string;
  group: 'move' | 'write' | 'act' | 'everywhere';
  label: string;
  /** What Help displays; falls back to the chord itself. */
  display?: KeyName | string[];
  chords: Chord[];
  available: boolean;
};

export const BINDINGS: Binding[] = [
  // ---- move around
  // Arrows come free from Ariakit's Composite and share the same active item,
  // so they are documented rather than reimplemented — an undocumented working
  // key is as much a gap as a documented broken one.
  { id: 'next', group: 'move', label: 'Next / previous conversation', display: ['J', 'K', '↑', '↓'],
    chords: [{ key: 'j' }, { key: 'k' }, { key: 'ArrowDown' }, { key: 'ArrowUp' }], available: true },
  { id: 'open', group: 'move', label: 'Open conversation', display: 'enter',
    chords: [{ key: 'Enter' }], available: true },
  { id: 'back', group: 'move', label: 'Back to the list', display: ['U'],
    chords: [{ key: 'u' }], available: true },
  { id: 'msg-nav', group: 'move', label: 'Next / previous message in thread', display: ['[', ']'],
    chords: [{ key: '[' }, { key: ']' }], available: true },
  { id: 'panes', group: 'move', label: 'Cycle panes', display: ['F6'],
    chords: [{ key: 'F6' }], available: true },
  { id: 'goto', group: 'move', label: 'Go to Inbox · Starred · Sent · Drafts', display: ['G', 'I S T D'],
    chords: [{ key: 'g', then: 'i' }], available: true },
  { id: 'account', group: 'move', label: 'Switch active account', display: 'account',
    chords: [{ key: '1', meta: true }], available: true },

  // ---- write (arrives with compose)
  { id: 'compose', group: 'write', label: 'Compose', display: ['C'],
    chords: [{ key: 'c' }], available: true },
  { id: 'reply', group: 'write', label: 'Reply · reply all · forward', display: ['R', 'A', 'F'],
    chords: [{ key: 'r' }], available: true },
  { id: 'send', group: 'write', label: 'Send', display: 'send',
    chords: [{ key: 'Enter', meta: true }], available: true },
  { id: 'send-later', group: 'write', label: 'Send later', display: 'sendLater',
    chords: [{ key: 'Enter', meta: true, shift: true }], available: false },
  { id: 'save-draft', group: 'write', label: 'Save draft', display: 'save',
    chords: [{ key: 's', meta: true }], available: false },
  { id: 'popout', group: 'write', label: 'Open in its own window', display: 'popout',
    chords: [{ key: 'o', meta: true, shift: true }], available: false },

  // ---- act on mail
  { id: 'archive', group: 'act', label: 'Archive', display: ['E'],
    chords: [{ key: 'e' }], available: true },
  { id: 'trash', group: 'act', label: 'Move to trash', display: ['#'],
    chords: [{ key: '#' }], available: true },
  { id: 'spam', group: 'act', label: 'Report spam', display: ['!'],
    chords: [{ key: '!' }], available: true },
  { id: 'star', group: 'act', label: 'Star', display: ['S'],
    chords: [{ key: 's' }], available: true },
  { id: 'snooze', group: 'act', label: 'Snooze this conversation', display: ['B'],
    chords: [{ key: 'b' }], available: true },
  { id: 'move-tag', group: 'act', label: 'Move to folder · tag', display: ['V', 'L'],
    chords: [{ key: 'v' }, { key: 'l' }], available: true },
  { id: 'read-unread', group: 'act', label: 'Mark read · unread', display: ['read', 'unread'],
    chords: [{ key: 'i', shift: true }, { key: 'u', shift: true }], available: true },
  { id: 'select', group: 'act', label: 'Select · extend selection', display: ['X', 'extend'],
    chords: [{ key: 'x' }], available: true },
  { id: 'undo', group: 'act', label: 'Undo the last thing', display: ['Z'],
    chords: [{ key: 'z' }], available: true },

  // ---- everywhere
  { id: 'search', group: 'everywhere', label: 'Search', display: ['/'],
    chords: [{ key: '/' }], available: true },
  { id: 'palette', group: 'everywhere', label: 'Command palette', display: 'palette',
    chords: [{ key: 'k', meta: true }], available: true },
  { id: 'help', group: 'everywhere', label: 'This list', display: ['?'],
    chords: [{ key: '?' }], available: true },
  { id: 'settings', group: 'everywhere', label: 'Settings', display: 'settings',
    chords: [{ key: ',', meta: true }], available: true },
];

const KEY_NAMES = new Set([
  'enter', 'account', 'send', 'sendLater', 'save', 'popout', 'read', 'unread',
  'extend', 'palette', 'settings',
]);

/** Resolve a binding's display into the labels Help renders. */
export function displayKeys(b: Binding): string[] {
  if (typeof b.display === 'string') return [key(b.display as KeyName)];
  if (Array.isArray(b.display)) {
    return b.display.map((d) => (KEY_NAMES.has(d) ? key(d as KeyName) : d));
  }
  return [];
}

export const GROUP_TITLES: Record<Binding['group'], string> = {
  move: 'Move around',
  write: 'Write',
  act: 'Act on mail',
  everywhere: 'Everywhere',
};
