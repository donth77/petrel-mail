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
  compose: () => void;
  reply: (all: boolean) => void;
  forward: () => void;
  snooze: () => void;
  select: () => void;
  extendSelection: (down: boolean) => void;
  clearSelection: () => void;
  openMove: () => void;
  openTag: () => void;
  toggleStar: () => void;
  moveToInbox: () => void;
  popOut: () => void;
  toggleReaderFull: () => void;
  findInMessage: () => void;
  undo: () => void;
};

/** True while a modal or a menu is up. Single-key commands must not reach the
 *  list behind it: archiving a conversation you cannot see, with the toast
 *  hidden under the dialog, is the worst possible version of a shortcut
 *  firing. A menu counts for the same reason — a right-click menu offering
 *  Archive was open over the row while E archived it underneath, and the menu
 *  then acted on a row that had already gone.
 *
 *  The `:not([hidden])` is load-bearing. Ariakit keeps every dialog and menu
 *  mounted and marks the closed ones `hidden`, so a bare `[role="dialog"]`
 *  matches even when nothing is open — a guard that would silently disable
 *  every shortcut in the app, permanently. */
function modalOpen(): boolean {
  return document.querySelector('[role="dialog"]:not([hidden]), [role="menu"]:not([hidden])') !== null;
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
      if (mod) {
        if (!e.altKey) {
          const k = e.key.toLowerCase();
          if (k === 'k') return e.preventDefault(), a.openPalette();
          if (k === ',') return e.preventDefault(), a.openSettings();
          if (/^[1-9]$/.test(e.key)) return e.preventDefault(), a.switchAccount(Number(e.key));
          // Free because modified keys stopped falling through to the
          // single-key commands; before that this forwarded the message.
          if (k === 'f') return e.preventDefault(), a.findInMessage();
        }
        // Anything else held with ⌘ or ctrl belongs to the system, and this
        // return is the whole of what makes that true.
        //
        // Without it every modified key fell through to the single-key commands
        // below: ⌘C opened the composer, ⌘A replied to all, ⌘V opened the move
        // picker, ⌘Z undid a triage action and ⌘F forwarded. Worse, they call
        // preventDefault, so the system action was not merely shadowed but
        // swallowed — you could not copy text out of a message at all.
        return;
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

      if ('eE#!sSzZIUvVlLcCrRaAfFbBxXJK'.includes(e.key)) {
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
        case 'c':
        case 'C':
          e.preventDefault();
          return a.compose();
        case 'r':
        case 'R':
          e.preventDefault();
          return a.reply(false);
        case 'a':
        case 'A':
          e.preventDefault();
          return a.reply(true);
        case 'f':
        case 'F':
          e.preventDefault();
          return a.forward();
        case 'x':
        case 'X':
          e.preventDefault();
          return a.select();
        case 'J':
          if (e.shiftKey) {
            e.preventDefault();
            return a.extendSelection(true);
          }
          break;
        case 'K':
          if (e.shiftKey) {
            e.preventDefault();
            return a.extendSelection(false);
          }
          break;
        case '\\':
          e.preventDefault();
          return a.toggleReaderFull();
        case 'Escape':
          // Only meaningful when something is selected; dialogs handle their
          // own Escape and never reach here.
          return a.clearSelection();
        case 'b':
        case 'B':
          e.preventDefault();
          return a.snooze();
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
        // Plain letters, deliberately below the shifted cases above: ⇧I marks
        // read and ⇧U marks unread, and both return before reaching here. The
        // same split `u` and ⇧U already live under.
        case 'i':
          e.preventDefault();
          return a.moveToInbox();
        case 'o':
        case 'O':
          e.preventDefault();
          return a.popOut();
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
