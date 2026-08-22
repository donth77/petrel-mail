import { useEffect, useRef } from 'react';

export type KeyActions = {
  openConversation: () => void;
  backToList: () => void;
  cyclePanes: (backwards: boolean) => void;
  goTo: (view: string) => void;
  switchAccount: (index: number) => void;
  openPalette: () => void;
  openHelp: () => void;
  openSettings: () => void;
  focusSearch: () => void;
  triage: (kind: import('./api').ActionKind) => void;
  openMove: () => void;
  openTag: () => void;
  toggleStar: () => void;
  undo: () => void;
};

/** True while a modal is up. Single-key commands must not reach the list
 *  behind it: archiving a conversation you cannot see, with the toast hidden
 *  under the dialog, is the worst possible version of a shortcut firing.
 *
 *  The `:not([hidden])` is load-bearing. Ariakit keeps every dialog mounted and
 *  marks the closed ones `hidden`, so a bare `[role="dialog"]` matches even
 *  when nothing is open — a guard that would silently disable every shortcut in
 *  the app, permanently. */
function modalOpen(): boolean {
  return document.querySelector('[role="dialog"]:not([hidden])') !== null;
}

/** Where a keystroke means text, not a command. */
function isTyping(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  if (!el) return false;
  return (
    el.tagName === 'INPUT' ||
    el.tagName === 'TEXTAREA' ||
    el.tagName === 'SELECT' ||
    el.isContentEditable
  );
}

const GOTO: Record<string, string> = { i: 'inbox', s: 'starred', t: 'sent', d: 'drafts' };

/**
 * One listener for every global shortcut, so bindings cannot drift apart across
 * components — and so the "single-key shortcuts pause while typing" rule is
 * enforced in one place rather than remembered in each handler.
 */
export function useKeyboard(actions: KeyActions) {
  const ref = useRef(actions);
  ref.current = actions;
  // A pending `g` waiting for its second key. Cleared on a timeout so a stray
  // press does not silently swallow the next keystroke minutes later.
  const chord = useRef<{ key: string; at: number } | null>(null);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const a = ref.current;
      const typing = isTyping(e.target);
      const mod = e.metaKey || e.ctrlKey;

      // Modified shortcuts work everywhere, including in a text field: ⌘K is
      // how you get *out* of one.
      if (mod && !e.altKey) {
        const k = e.key.toLowerCase();
        if (k === 'k') return e.preventDefault(), a.openPalette();
        if (k === ',') return e.preventDefault(), a.openSettings();
        if (/^[1-9]$/.test(e.key)) return e.preventDefault(), a.switchAccount(Number(e.key));
      }

      if (typing) {
        if (e.key === 'Escape') (e.target as HTMLElement).blur();
        return;
      }

      // Escape still belongs to the dialog itself, which handles it; everything
      // else stops here.
      if (modalOpen()) return;

      // A pending chord takes the next key, if it arrives promptly.
      if (chord.current) {
        const pending = chord.current;
        chord.current = null;
        if (Date.now() - pending.at < 1500 && pending.key === 'g') {
          const view = GOTO[e.key.toLowerCase()];
          if (view) {
            e.preventDefault();
            a.goTo(view);
            return;
          }
        }
      }

      if ('eE#!sSzZIUvVlL'.includes(e.key)) {
        void import('./api').then(({ api }) =>
          api.log(
            JSON.stringify({
              kind: 'key',
              key: e.key,
              shift: e.shiftKey,
              target: (e.target as HTMLElement | null)?.className ?? String(e.target),
            }),
          ),
        );
      }

      // Triage. Single keys, so they yield to text fields like everything else.
      switch (e.key) {
        case 'e':
        case 'E':
          e.preventDefault();
          return a.triage('archive');
        case '#':
          e.preventDefault();
          return a.triage('trash');
        case '!':
          e.preventDefault();
          return a.triage('spam');
        case 's':
        case 'S':
          e.preventDefault();
          return a.toggleStar();
        case 'v':
        case 'V':
          e.preventDefault();
          return a.openMove();
        case 'l':
        case 'L':
          e.preventDefault();
          return a.openTag();
        case 'z':
        case 'Z':
          e.preventDefault();
          return a.undo();
        case 'I':
          if (e.shiftKey) {
            e.preventDefault();
            return a.triage('mark_read');
          }
          break;
        case 'U':
          if (e.shiftKey) {
            e.preventDefault();
            return a.triage('mark_unread');
          }
          break;
      }

      switch (e.key) {
        case 'g':
        case 'G':
          chord.current = { key: 'g', at: Date.now() };
          return;
        case 'Enter':
          e.preventDefault();
          return a.openConversation();
        case 'u':
        case 'U':
          e.preventDefault();
          return a.backToList();
        case 'F6':
          e.preventDefault();
          return a.cyclePanes(e.shiftKey);
        case '/':
          e.preventDefault();
          return a.focusSearch();
        case '?':
          e.preventDefault();
          return a.openHelp();
      }
    };

    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);
}
