/**
 * Shortcut labels rendered from the running platform, never hardcoded.
 *
 * macOS concatenates modifier glyphs in a fixed order (⇧⌘O); Windows and Linux
 * spell them out and join with "+", in their own order (Ctrl+Shift+O). Same
 * binding, two vocabularies — and showing the wrong one is worse than showing
 * none, because it teaches a keystroke that does nothing. (docs 06)
 */
const isMac =
  typeof navigator !== 'undefined' &&
  /mac/i.test(
    (navigator as { userAgentData?: { platform?: string } }).userAgentData?.platform ??
      navigator.userAgent,
  );

const MAC = {
  enter: '↵',
  account: '⌘1…9',
  send: '⌘↵',
  sendLater: '⌘⇧↵',
  save: '⌘S',
  popout: '⇧⌘O',
  read: '⇧I',
  unread: '⇧U',
  extend: '⇧J ⇧K',
  find: '⌘F',
  palette: '⌘K',
  settings: '⌘,',
} as const;

const PC: Record<keyof typeof MAC, string> = {
  enter: 'Enter',
  account: 'Ctrl+1…9',
  send: 'Ctrl+Enter',
  sendLater: 'Ctrl+Shift+Enter',
  save: 'Ctrl+S',
  popout: 'Ctrl+Shift+O',
  read: 'Shift+I',
  unread: 'Shift+U',
  extend: 'Shift+J Shift+K',
  find: 'Ctrl+F',
  palette: 'Ctrl+K',
  settings: 'Ctrl+,',
};

export type KeyName = keyof typeof MAC;

export function key(name: KeyName): string {
  return isMac ? MAC[name] : PC[name];
}
