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
import type { StringId } from './strings';

export type Chord = { key: string; shift?: boolean; meta?: boolean; then?: string };

export type Binding = {
  id: string;
  group: 'move' | 'write' | 'act' | 'everywhere';
  label: StringId;
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
  { id: 'next', group: 'move', label: 'sc-next', display: ['J', 'K', '↑', '↓'],
    chords: [{ key: 'j' }, { key: 'k' }, { key: 'ArrowDown' }, { key: 'ArrowUp' }], available: true },
  { id: 'open', group: 'move', label: 'sc-open', display: 'enter',
    chords: [{ key: 'Enter' }], available: true },
  { id: 'back', group: 'move', label: 'sc-back', display: ['U'],
    chords: [{ key: 'u' }], available: true },
  { id: 'msg-nav', group: 'move', label: 'sc-msg-nav', display: ['[', ']'],
    chords: [{ key: '[' }, { key: ']' }], available: true },
  { id: 'panes', group: 'move', label: 'sc-panes', display: ['F6'],
    chords: [{ key: 'F6' }], available: true },
  { id: 'goto', group: 'move', label: 'sc-goto', display: ['G', 'I S T D'],
    chords: [{ key: 'g', then: 'i' }], available: true },
  { id: 'account', group: 'move', label: 'sc-account', display: 'account',
    chords: [{ key: '1', meta: true }], available: true },

  // ---- write (arrives with compose)
  { id: 'compose', group: 'write', label: 'sc-compose', display: ['C'],
    chords: [{ key: 'c' }], available: true },
  { id: 'reply', group: 'write', label: 'sc-reply', display: ['R', 'A', 'F'],
    chords: [{ key: 'r' }], available: true },
  { id: 'send', group: 'write', label: 'sc-send', display: 'send',
    chords: [{ key: 'Enter', meta: true }], available: true },
  { id: 'send-later', group: 'write', label: 'sc-send-later', display: 'sendLater',
    chords: [{ key: 'Enter', meta: true, shift: true }], available: true },
  { id: 'save-draft', group: 'write', label: 'sc-save-draft', display: 'save',
    chords: [{ key: 's', meta: true }], available: true },
  { id: 'popout', group: 'write', label: 'sc-popout', display: 'popout',
    chords: [{ key: 'o', meta: true, shift: true }], available: true },

  { id: 'reader-full', group: 'move', label: 'sc-reader-full', display: ['\\'],
    chords: [{ key: '\\' }], available: true },
  { id: 'reader-scroll', group: 'move', label: 'sc-reader-scroll',
    display: ['Space', '⇧Space'], chords: [{ key: ' ' }], available: true },
  { id: 'find-in-message', group: 'move', label: 'sc-find-in-message',
    display: 'find', chords: [{ key: 'f', meta: true }], available: true },

  // ---- act on mail
  { id: 'archive', group: 'act', label: 'sc-archive', display: ['E'],
    chords: [{ key: 'e' }], available: true },
  { id: 'trash', group: 'act', label: 'sc-trash', display: ['#'],
    chords: [{ key: '#' }], available: true },
  { id: 'spam', group: 'act', label: 'sc-spam', display: ['!'],
    chords: [{ key: '!' }], available: true },
  { id: 'star', group: 'act', label: 'sc-star', display: ['S'],
    chords: [{ key: 's' }], available: true },
  { id: 'snooze', group: 'act', label: 'sc-snooze', display: ['B'],
    chords: [{ key: 'b' }], available: true },
  { id: 'move-tag', group: 'act', label: 'sc-move-tag', display: ['V', 'L'],
    chords: [{ key: 'v' }, { key: 'l' }], available: true },
  // The way back out of Archive, Trash and Spam. Plain I, beside ⇧I for
  // "mark read" — the same split U already has.
  { id: 'move-inbox', group: 'act', label: 'sc-move-inbox', display: ['I'],
    chords: [{ key: 'i' }], available: true },
  { id: 'pop-out', group: 'act', label: 'sc-pop-out', display: ['O'],
    chords: [{ key: 'o' }], available: true },
  { id: 'read-unread', group: 'act', label: 'sc-read-unread', display: ['read', 'unread'],
    chords: [{ key: 'i', shift: true }, { key: 'u', shift: true }], available: true },
  { id: 'select', group: 'act', label: 'sc-select', display: ['X', 'extend'],
    chords: [{ key: 'x' }], available: true },
  { id: 'undo', group: 'act', label: 'sc-undo', display: ['Z'],
    chords: [{ key: 'z' }], available: true },

  // ---- everywhere
  { id: 'search', group: 'everywhere', label: 'sc-search', display: ['/'],
    chords: [{ key: '/' }], available: true },
  { id: 'palette', group: 'everywhere', label: 'sc-palette', display: 'palette',
    chords: [{ key: 'k', meta: true }], available: true },
  { id: 'help', group: 'everywhere', label: 'sc-help', display: ['?'],
    chords: [{ key: '?' }], available: true },
  { id: 'settings', group: 'everywhere', label: 'sc-settings', display: 'settings',
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

export const GROUP_TITLES: Record<Binding['group'], StringId> = {
  move: 'sc-group-move',
  write: 'sc-group-write',
  act: 'sc-group-act',
  everywhere: 'sc-group-everywhere',
};
