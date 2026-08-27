import { describe, expect, it } from 'vitest';
import { MENU, MENU_COMMANDS, type MenuNode } from './menu';
import { STRING_IDS } from './string-ids';
import { t } from './strings';

/* A menu bar cannot be read back out of the DOM, and this environment grants
   neither Accessibility nor Screen Recording, so nothing can look at one. What
   is checkable is the description the menu is built from: that every item has
   a name people will actually see, that every command has somewhere to go, and
   that no two accelerators fight. Those are the failures that would otherwise
   only show up as a blank row or a dead key in a shipped build. */

function walk(nodes: MenuNode[], depth = 0): { node: MenuNode; depth: number }[] {
  return nodes.flatMap((node) =>
    node.role === 'submenu'
      ? [{ node, depth }, ...walk(node.items, depth + 1)]
      : [{ node, depth }],
  );
}

const all = walk(MENU);

describe('the menu bar', () => {
  it('names every item from the bundle', () => {
    const labelled = all.filter(({ node }) => node.role !== 'separator');
    expect(labelled.length).toBeGreaterThan(0);
    for (const { node } of labelled) {
      if (node.role === 'separator') continue;
      expect(STRING_IDS).toContain(node.label);
      // t() falls back to the id when a string is missing. An id in a menu bar
      // is the exact failure the Fluent migration was supposed to end.
      expect(t(node.label)).not.toBe(node.label);
      expect(t(node.label).trim()).not.toBe('');
    }
  });

  it('sends every command somewhere', () => {
    const commands = all
      .map(({ node }) => (node.role === 'command' ? node.command : null))
      .filter((c): c is (typeof MENU_COMMANDS)[number] => c !== null);
    // The point of the check: an item that names a command nothing handles is
    // an item that silently does nothing when clicked.
    for (const command of commands) expect(MENU_COMMANDS).toContain(command);
    expect(commands.length).toBeGreaterThan(0);
  });

  it('gives no two items the same key', () => {
    const keys = all
      .map(({ node }) => (node.role === 'command' ? node.accelerator : undefined))
      .filter((a): a is string => Boolean(a));
    expect(new Set(keys).size).toBe(keys.length);
    // Modifier+key, in the form muda parses. A typo here is a menu item that
    // builds and then never responds to its own printed shortcut.
    for (const key of keys) expect(key).toMatch(/^(CmdOrCtrl|Shift|Alt)(\+(CmdOrCtrl|Shift|Alt))*\+.+$/);
  });

  it('carries the two shortcuts the plan names', () => {
    const commands = all.filter(({ node }) => node.role === 'command');
    const find = (command: string) =>
      commands.find(({ node }) => node.role === 'command' && node.command === command)?.node;
    const compose = find('new-message');
    const settings = find('settings');
    expect(compose && compose.role === 'command' && compose.accelerator).toBe('CmdOrCtrl+N');
    expect(settings && settings.role === 'command' && settings.accelerator).toBe('CmdOrCtrl+,');
  });

  it('keeps a full Edit menu, which is what dictation and the emoji picker need', () => {
    const edit = MENU.find((n) => n.role === 'submenu' && n.label === 'menubar-edit');
    expect(edit?.role).toBe('submenu');
    const natives =
      edit?.role === 'submenu'
        ? edit.items.map((i) => (i.role === 'native' ? i.native : null)).filter(Boolean)
        : [];
    expect(natives).toEqual(['Undo', 'Redo', 'Cut', 'Copy', 'Paste', 'SelectAll']);
  });

  it('puts the app menu first and Window last, as macOS expects', () => {
    const titles = MENU.map((n) => (n.role === 'submenu' ? n.label : null));
    expect(titles[0]).toBe('app-name');
    expect(titles[titles.length - 1]).toBe('menubar-window');
    // Only AppKit can draw the window list, and it draws it into whichever
    // submenu is handed to it. Exactly one, or the list lands nowhere.
    const owned = MENU.filter((n) => n.role === 'submenu' && n.windowMenu);
    expect(owned.length).toBe(1);
  });

  it('offers every value each setting can take', () => {
    const values = (setting: string) =>
      all
        .map(({ node }) => (node.role === 'choice' && node.setting === setting ? node.value : null))
        .filter(Boolean);
    // A theme or a density added to settings.tsx and not to the menu is a menu
    // that cannot show the state the app is in.
    expect(values('theme')).toEqual(['light', 'dark', 'system']);
    expect(values('density')).toEqual(['relaxed', 'compact']);
  });

  it('leaves no submenu empty', () => {
    for (const { node } of all) {
      if (node.role === 'submenu') expect(node.items.length).toBeGreaterThan(0);
    }
  });
});
