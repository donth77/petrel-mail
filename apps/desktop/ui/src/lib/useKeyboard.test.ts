import { describe, expect, it } from 'vitest';
import {
  accountSwitchAllowed,
  escapeLeavesField,
  mailKeyDecision,
  placeOf,
  singleKeysAllowed,
  textSizeStep,
} from './useKeyboard';

/**
 * The rules for when a single key is a mail command. They decide whether a
 * letter typed into a message lands as a letter or archives the conversation
 * behind it, so they are pinned here rather than left to the handler's shape.
 */
describe('single-key commands while a message is being written', () => {
  it('run from the rail, the list and the reader', () => {
    for (const key of ['e', 'a', 's', '#', 'j', 'Escape', 'Enter']) {
      expect(singleKeysAllowed(true, 'pane', key), key).toBe(true);
    }
  });

  it('never run from inside the composer', () => {
    // A button in the composer has focus after a Tab or a click: "see" there
    // starred and archived the open conversation, then archived the next.
    for (const key of ['e', 'a', 's', '#', 'Escape', 'Enter']) {
      expect(singleKeysAllowed(true, 'composer', key), key).toBe(false);
    }
  });

  it('never run from the page itself while the composer is open', () => {
    // Where focus falls when it leaves a field: Escape in Subject, or a click
    // on Attach in WebKit. The next letters were meant for the message.
    expect(singleKeysAllowed(true, 'elsewhere', 'a')).toBe(false);
    expect(singleKeysAllowed(true, 'elsewhere', 'e')).toBe(false);
  });

  it('run from anywhere when no composer is open', () => {
    expect(singleKeysAllowed(false, 'elsewhere', 'e')).toBe(true);
    expect(singleKeysAllowed(false, 'pane', 'e')).toBe(true);
  });

  it('let F6 move between panes from anywhere, the composer included', () => {
    expect(singleKeysAllowed(true, 'composer', 'F6')).toBe(true);
    expect(singleKeysAllowed(true, 'elsewhere', 'F6')).toBe(true);
  });
});

describe('Escape in a text field', () => {
  it('leaves the search field', () => {
    expect(escapeLeavesField('pane')).toBe(true);
    expect(escapeLeavesField('elsewhere')).toBe(true);
  });

  it('keeps focus in the composer', () => {
    expect(escapeLeavesField('composer')).toBe(false);
  });
});

describe('⌘1–⌘9', () => {
  it('switches accounts in the ordinary case', () => {
    expect(accountSwitchAllowed(false, false)).toBe(true);
  });

  it('does nothing under an open dialog', () => {
    // Empty Trash, opened on one account, confirmed after ⌘2: the other
    // account's Trash was expunged.
    expect(accountSwitchAllowed(true, false)).toBe(false);
  });

  it('does nothing while a recipient is being typed in the composer', () => {
    expect(accountSwitchAllowed(false, true)).toBe(false);
  });
});

/** A stand-in for an element: `closest` finds the classes it sits inside. */
function inside(...classes: string[]): Element {
  return {
    closest: (selector: string) =>
      selector.split(',').some((part) => classes.includes(part.trim().replace(/^\./, '')))
        ? {}
        : null,
  } as unknown as Element;
}

describe('placeOf', () => {
  it('names the pane, the composer, or somewhere else', () => {
    expect(placeOf(inside('rail'), null)).toBe('pane');
    expect(placeOf(inside('list-pane'), null)).toBe('pane');
    expect(placeOf(inside('reader'), null)).toBe('pane');
    expect(placeOf(inside('compose', 'rich-tools'), null)).toBe('composer');
    // The page itself, where a click on the composer's From line drops focus.
    expect(placeOf(inside(), null)).toBe('elsewhere');
  });

  it('places a key the message forwarded where focus is: in the reader', () => {
    // The frame's keys are replayed on the window, which is no element and
    // sits in no pane. Read from the target alone, every key pressed in a
    // message while a reply was open was refused.
    const theWindow = {} as EventTarget;
    expect(placeOf(theWindow, inside('reader'))).toBe('pane');
    expect(placeOf(theWindow, null)).toBe('elsewhere');
  });
});

describe('mailKeyDecision: the one rule every single key goes through', () => {
  const at = (place: 'pane' | 'composer' | 'elsewhere', over = {}) => ({
    typing: false,
    modal: false,
    composer: true,
    place,
    ...over,
  });

  it('lets J and K, [ and ] walk the panes while a message is being written', () => {
    for (const key of ['j', 'k', '[', ']', 'ArrowDown']) {
      expect(mailKeyDecision(at('pane'), key), key).toBe(true);
    }
  });

  it('keeps them, like every other letter, out of the composer and off the page', () => {
    // "Thanks so much" typed on the toolbar walked the list at its k, then
    // starred, popped out and replaced the message being written.
    for (const key of ['j', 'k', '[', ']', 't', 's', 'o', 'c']) {
      expect(mailKeyDecision(at('composer'), key), key).toBe(false);
      expect(mailKeyDecision(at('elsewhere'), key), key).toBe(false);
    }
  });

  it('never acts on a key typed into a field, or under a dialog or a menu', () => {
    expect(mailKeyDecision(at('pane', { typing: true }), 'j')).toBe(false);
    expect(mailKeyDecision(at('pane', { modal: true }), 'j')).toBe(false);
    expect(mailKeyDecision(at('elsewhere', { composer: false, modal: true }), 'e')).toBe(false);
  });

  it('acts from anywhere when nothing is being written', () => {
    expect(mailKeyDecision(at('elsewhere', { composer: false }), 'j')).toBe(true);
  });
});

/* Read through Vite rather than node:fs, as language-switch.test.ts does: the
   UI has no Node types, and the bundler already has every file in hand. */
const SOURCES = import.meta.glob('../**/*.{ts,tsx}', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;

describe('every key listener on the window', () => {
  // Listeners that never act on an unmodified key, and why.
  const EXEMPT: Record<string, string> = {
    'Compose.tsx': 'the composer\'s own ⌘ shortcuts',
    'useDrag.ts': 'Escape abandons a drag',
    // The menu is itself the modal mailKeyAllowed refuses, so asking that
    // rule would make Escape unable to close it.
    'RecipientMenu.tsx': 'Escape dismisses the address menu',
    // Only listens: a key means the press that dismissed the menu is over.
    'recipient-menu.ts': 'a key lets go of the click a dismissing press held',
  };

  it('decides single keys through mailKeyAllowed', () => {
    // J and K lived in the list's own listener and [ and ] in the reader's,
    // and neither asked the composer rule the rest of the keys obey.
    const listening: string[] = [];
    for (const [path, src] of Object.entries(SOURCES)) {
      if (/\.test\.tsx?$/.test(path)) continue;
      const code = src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/^\s*\/\/.*$/gm, '');
      if (!/\b(?:window|document)\.addEventListener\(\s*['"]keydown['"]/.test(code)) continue;
      const name = path.split('/').pop()!;
      listening.push(name);
      if (EXEMPT[name]) continue;
      expect(code.includes('mailKeyAllowed('), name).toBe(true);
    }
    // The glob found them: a pattern that matched nothing would pass vacuously.
    expect(listening).toEqual(
      expect.arrayContaining(['MessageList.tsx', 'Reader.tsx', 'useKeyboard.ts']),
    );
  });
});

describe('each pane', () => {
  it('takes focus from a click on its own text', () => {
    // A click on the reader's subject, the list's header or (in WebKit, which
    // does not focus a button it clicks) a mailbox in the rail left focus on
    // the page, which is no pane: while a message was being written, every
    // key after it was refused. Focusable at -1, a pane takes the focus a
    // click on its non-focusable parts would otherwise drop, and Tab passes it by.
    const roots: Record<string, RegExp> = {
      rail: /<nav\s[^>]*className="rail"[^>]*>/,
      'list-pane': /<div\s[^>]*className="list-pane"[^>]*>/,
      reader: /<section\s[^>]*className="reader"[^>]*>/g,
    };
    const tags = (re: RegExp) =>
      Object.entries(SOURCES)
        .filter(([path]) => !/\.test\.tsx?$/.test(path))
        .flatMap(([, src]) => src.match(new RegExp(re.source, 'g')) ?? []);
    for (const [pane, re] of Object.entries(roots)) {
      const found = tags(re);
      expect(found.length, pane).toBeGreaterThan(0);
      for (const tag of found) expect(tag, pane).toMatch(/tabIndex=\{-1\}/);
    }
  });
});

describe('the reading text size keys', () => {
  it('grows on the key that carries = and +, shifted or not', () => {
    // ⌘+ is ⌘⇧= on most keyboards, and the keypad's plus reports + too.
    expect(textSizeStep('=')).toBe(1);
    expect(textSizeStep('+')).toBe(1);
  });

  it('shrinks on minus and goes back to the default on zero', () => {
    expect(textSizeStep('-')).toBe(-1);
    expect(textSizeStep('0')).toBe(0);
  });

  it('leaves every other key alone, the account digits most of all', () => {
    // ⌘1 to ⌘9 switch account; ⌘0 is the only digit this claims.
    for (const key of ['1', '9', '_', ')', 'n', 'Enter']) expect(textSizeStep(key)).toBeNull();
  });
});
