/* The native menu bar.
 *
 * Built here rather than in Rust for two reasons. The labels come from the
 * Fluent bundle, which lives in the window — a menu built in the shell would
 * need every string shipped across the IPC seam or duplicated in Rust, and a
 * duplicated string is a string that will one day disagree with itself. And the
 * items run the *same functions* the keyboard and the palette run, as closures,
 * not as an event id that has to be matched back to a command at the other end.
 * A menu is a way to find a command; like the palette, it is not a second
 * implementation of one (see lib/commands.ts).
 *
 * What was here before: Tauri installs a default macOS menu when an app sets
 * none, so Petrel has never been menu-less — it has had Tauri's generic one,
 * with a File menu holding only Close and a View menu holding only Full Screen.
 * `install` logs what it replaced, so that claim can be checked rather than
 * believed.
 *
 * macOS only in effect. The structure is what a Mac app is expected to have —
 * the app menu first, Window last — and Windows and Linux will want their own
 * arrangement when they arrive (docs 16 §5). Nothing here breaks on them; the
 * predefined items that those platforms lack are simply absent from the menu
 * they draw.
 */

import { useEffect, useRef } from 'react';
import type {
  CheckMenuItem,
  CheckMenuItemOptions,
  MenuItemOptions,
  PredefinedMenuItemOptions,
  Submenu,
  SubmenuOptions,
} from '@tauri-apps/api/menu';
import { api } from './api';
import { t, type StringId } from './strings';
import type { Settings } from './settings';
import { ISSUES_URL, SOURCE_URL } from './project';

/** The items macOS implements itself. Their accelerators, their behavior and
 *  their place in the responder chain come with them: an Edit menu of these is
 *  what makes dictation and the emoji picker work inside a webview. */
type NativeItem = Extract<PredefinedMenuItemOptions['item'], string>;

/** A command Petrel answers itself. Adding a name here forces the hook below
 *  to provide a handler for it — the compiler, not a code review, is what
 *  stops a menu item that does nothing. */
export const MENU_COMMANDS = ['new-message', 'settings', 'help', 'find'] as const;
export type MenuCommand = (typeof MENU_COMMANDS)[number];

/** The settings the View menu drives. Both already exist in Settings ›
 *  Appearance; the menu is another way to reach them, not another copy. */
type Choice =
  | { setting: 'theme'; value: Settings['theme'] }
  | { setting: 'density'; value: Settings['density'] };

export type MenuNode =
  | { role: 'separator' }
  | { role: 'native'; native: NativeItem; label: StringId }
  | { role: 'about'; label: StringId }
  | { role: 'command'; command: MenuCommand; label: StringId; accelerator?: string }
  | ({ role: 'choice'; label: StringId } & Choice)
  /** Opens an address in the browser. Not a command: there is no state to
   *  change and nothing for the window to do, so routing it through the
   *  command table would add a hop that carries nothing. */
  | { role: 'link'; label: StringId; url: string }
  | {
      role: 'submenu';
      label: StringId;
      items: MenuNode[];
      windowMenu?: true;
      helpMenu?: true;
    };

/**
 * The menu, as data.
 *
 * Kept declarative so it can be asserted on: menu.test.ts walks this and checks
 * that every label exists in the bundle, every command has a handler, and no
 * two accelerators collide. None of that is observable from a running menu bar
 * without a person looking at one.
 */
export const MENU: MenuNode[] = [
  {
    role: 'submenu',
    label: 'app-name',
    items: [
      { role: 'about', label: 'menubar-about' },
      { role: 'separator' },
      // The same pane ⌘, has always opened. The accelerator now belongs to the
      // menu: macOS gives its key equivalents first refusal, so the window's
      // own ⌘, handler no longer sees the keystroke. Both lead to one place.
      { role: 'command', command: 'settings', label: 'menubar-settings', accelerator: 'CmdOrCtrl+,' },
      { role: 'separator' },
      { role: 'native', native: 'Services', label: 'menubar-services' },
      { role: 'separator' },
      { role: 'native', native: 'Hide', label: 'menubar-hide' },
      { role: 'native', native: 'HideOthers', label: 'menubar-hide-others' },
      { role: 'native', native: 'ShowAll', label: 'menubar-show-all' },
      { role: 'separator' },
      { role: 'native', native: 'Quit', label: 'menubar-quit' },
    ],
  },
  {
    role: 'submenu',
    label: 'menubar-file',
    items: [
      { role: 'command', command: 'new-message', label: 'menubar-new-message', accelerator: 'CmdOrCtrl+N' },
      { role: 'separator' },
      { role: 'native', native: 'CloseWindow', label: 'menubar-close-window' },
    ],
  },
  {
    // Load-bearing beyond the obvious. Without these, macOS has nowhere to send
    // the editing commands dictation and the emoji picker issue, and both
    // misbehave inside the webview.
    //
    // They look inert more often than they are. Every one carries the real
    // AppKit selector with no target, so it travels the responder chain and
    // works wherever this app has text — the composer, the search field, a
    // folder being renamed, a selection in the reader — and does nothing
    // elsewhere. What it will not do is *say* so: the menu library calls
    // setAutoenablesItems(false) on every menu it builds, which is exactly the
    // macOS mechanism that would grey them out when nothing can respond. So
    // they stay black and clickable whether or not they apply. Reaching past
    // Tauri into the NSMenu to turn validation back on would couple this file
    // to another library's internals for the sake of some dimming.
    //
    // Find is the one item here Petrel implements itself, and it is the reason
    // this menu is worth opening: ⌘F searches the whole open conversation,
    // across every message frame in it, and until now existed only for people
    // who already knew to press it.
    role: 'submenu',
    label: 'menubar-edit',
    items: [
      { role: 'native', native: 'Undo', label: 'menubar-undo' },
      { role: 'native', native: 'Redo', label: 'menubar-redo' },
      { role: 'separator' },
      { role: 'native', native: 'Cut', label: 'menubar-cut' },
      { role: 'native', native: 'Copy', label: 'menubar-copy' },
      { role: 'native', native: 'Paste', label: 'menubar-paste' },
      { role: 'native', native: 'SelectAll', label: 'menubar-select-all' },
      { role: 'separator' },
      { role: 'command', command: 'find', label: 'menubar-find', accelerator: 'CmdOrCtrl+F' },
    ],
  },
  {
    role: 'submenu',
    label: 'menubar-view',
    items: [
      {
        role: 'submenu',
        label: 'appearance-theme',
        items: [
          { role: 'choice', setting: 'theme', value: 'light', label: 'theme-light' },
          { role: 'choice', setting: 'theme', value: 'dark', label: 'theme-dark' },
          { role: 'choice', setting: 'theme', value: 'system', label: 'theme-system' },
        ],
      },
      {
        role: 'submenu',
        label: 'appearance-density',
        items: [
          { role: 'choice', setting: 'density', value: 'relaxed', label: 'density-relaxed' },
          { role: 'choice', setting: 'density', value: 'compact', label: 'density-compact' },
        ],
      },
      { role: 'separator' },
      // Kept, not added. Tauri's default View menu held this one item, and
      // ⌃⌘F exists only because a menu item claims it — replacing the menu
      // without it would have quietly taken full screen off the keyboard.
      { role: 'native', native: 'Fullscreen', label: 'menubar-fullscreen' },
    ],
  },
  {
    // `windowMenu` hands this one to AppKit, which appends the list of open
    // windows to it. That list is the whole reason the menu is worth having
    // with a popped-out composer or message on screen, and it is not something
    // this file can draw for itself.
    role: 'submenu',
    label: 'menubar-window',
    windowMenu: true,
    items: [
      { role: 'native', native: 'Minimize', label: 'menubar-minimize' },
      { role: 'native', native: 'Maximize', label: 'menubar-zoom' },
      { role: 'separator' },
    ],
  },
  {
    // Last, which is where macOS expects it, and marked so AppKit takes it:
    // a Help menu it owns gets the search box that walks the menus for you.
    // Tauri's default menu had an empty Help submenu, so replacing that menu
    // took the search box away; this puts it back and gives it something to
    // hold.
    role: 'submenu',
    label: 'menubar-help',
    helpMenu: true,
    items: [
      // No accelerator, deliberately. ? already opens this pane, and a menu
      // item claiming that key would take it from every text field in the app
      // — menus get first refusal on their key equivalents, which is the same
      // mechanism that nearly cost Toggle Full Screen, running the other way.
      { role: 'command', command: 'help', label: 'menubar-petrel-help' },
      { role: 'separator' },
      { role: 'link', label: 'menubar-report-issue', url: ISSUES_URL },
      { role: 'link', label: 'menubar-view-source', url: SOURCE_URL },
    ],
  },
];

/** What the window has to supply for the menu to mean anything. The two
 *  settings are read as well as written: a checkmark that does not follow the
 *  Appearance pane is worse than no checkmark. */
export type MenuBindings = {
  /** The same draft the C key and the palette open. */
  newMessage: () => void;
  /** The same pane ⌘, opens. */
  openSettings: () => void;
  /** The same pane ? opens. */
  openHelp: () => void;
  /** The same bar ⌘F opens, with the same rule about when there is anything
   *  to find in. */
  find: () => void;
  theme: Settings['theme'];
  density: Settings['density'];
  setTheme: (value: Settings['theme']) => void;
  setDensity: (value: Settings['density']) => void;
};

type Built = CheckMenuItem | Submenu | MenuItemOptions | SubmenuOptions | PredefinedMenuItemOptions;

type Tauri = typeof import('@tauri-apps/api/menu');

type BuildContext = {
  tauri: Tauri;
  /** Current values, so the checkmarks start honest. */
  state: { theme: string; density: string };
  run: (command: MenuCommand) => void;
  choose: (setting: 'theme' | 'density', value: string) => void;
  /** Every check item, so their marks can be corrected without rebuilding the
   *  menu. Keyed `setting:value`. */
  checks: Map<string, CheckMenuItem>;
  /** The submenu AppKit should own, once the menu is installed. */
  windowMenu: Submenu | null;
  /** Likewise, the one it should treat as Help. */
  helpMenu: Submenu | null;
};

async function build(node: MenuNode, ctx: BuildContext): Promise<Built> {
  switch (node.role) {
    case 'separator':
      return { item: 'Separator' };
    case 'native':
      return { item: node.native, text: t(node.label) };
    case 'about':
      // Null metadata means the standard macOS panel, which reads the name,
      // version and icon out of the bundle. Passing our own would be a second
      // place for the version to live and a second place for it to be wrong.
      return { item: { About: null }, text: t(node.label) };
    case 'command':
      return {
        text: t(node.label),
        accelerator: node.accelerator,
        action: () => ctx.run(node.command),
      };
    case 'link':
      return {
        text: t(node.label),
        action: () => {
          void api.log(JSON.stringify({ kind: 'menu', command: `open:${node.url}` }));
          void api.openExternal(node.url);
        },
      };
    case 'choice': {
      const key = `${node.setting}:${node.value}`;
      const options: CheckMenuItemOptions = {
        text: t(node.label),
        checked: ctx.state[node.setting] === node.value,
        action: () => ctx.choose(node.setting, node.value),
      };
      const item = await ctx.tauri.CheckMenuItem.new(options);
      ctx.checks.set(key, item);
      return item;
    }
    case 'submenu': {
      const items: Built[] = [];
      for (const child of node.items) items.push(await build(child, ctx));
      if (!node.windowMenu && !node.helpMenu) return { text: t(node.label), items };
      // AppKit needs the real object, not a description of one.
      const submenu = await ctx.tauri.Submenu.new({ text: t(node.label), items });
      if (node.windowMenu) ctx.windowMenu = submenu;
      else ctx.helpMenu = submenu;
      return submenu;
    }
  }
}

/** The submenu titles of a menu, and how many items each holds, read back out
 *  of the OS rather than assumed from what we asked for. A menu bar is as
 *  opaque from outside as a webview is (AGENTS.md), and this is the only view
 *  into one that does not need a person looking at the screen. */
async function describe(menu: { items: () => Promise<unknown[]> }): Promise<string[]> {
  const out: string[] = [];
  for (const item of await menu.items()) {
    const submenu = item as { text?: () => Promise<string>; items?: () => Promise<unknown[]> };
    if (!submenu.text) continue;
    const title = await submenu.text();
    const count = submenu.items ? (await submenu.items()).length : 0;
    out.push(`${title}(${count})`);
  }
  return out;
}

/**
 * Builds the menu and hands it to the OS.
 *
 * Returns the check items so their marks can be kept in step, and logs both the
 * menu it built and the menu it replaced.
 */
async function install(ctx: BuildContext): Promise<Map<string, CheckMenuItem>> {
  const items: Built[] = [];
  for (const node of MENU) items.push(await build(node, ctx));
  const menu = await ctx.tauri.Menu.new({ items });
  const previous = await menu.setAsAppMenu();
  // Told to AppKit after the menu is in place; before, and there is no menu
  // bar for it to attach the window list to.
  await ctx.windowMenu?.setAsWindowsMenuForNSApp();
  // Without this macOS falls back to matching the *localized* word "Help",
  // which is exactly the guess that stops working in the six languages this
  // app now speaks. Saying which submenu it is does not depend on the title.
  await ctx.helpMenu?.setAsHelpMenuForNSApp();
  const replaced = previous ? await describe(previous) : [];
  void api.log(JSON.stringify({ kind: 'menu-installed', menu: await describe(menu), replaced }));
  // The old menu is nobody's now. Left alive it holds a resource per item for
  // the life of the process, and the language picker rebuilds this menu every
  // time it is used.
  if (previous) await previous.close();
  return ctx.checks;
}

/**
 * Puts Petrel's menu bar up, and keeps its checkmarks true.
 *
 * Built once. The handlers dispatch through a ref, so they always call the
 * current render's functions rather than the ones that existed at mount —
 * which is what lets the menu be installed once instead of on every keystroke
 * that changes the app's state.
 */
export function useAppMenu(bindings: MenuBindings): void {
  const ref = useRef(bindings);
  ref.current = bindings;
  const checks = useRef<Map<string, CheckMenuItem> | null>(null);

  useEffect(() => {
    // Not under Tauri: `npm run dev` in a browser, and the harness, both run
    // this bundle with no menu API behind it. Nothing to do, and nothing worth
    // saying about it.
    if (typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) return;
    let live = true;

    const mark = (theme: string, density: string) => {
      const map = checks.current;
      if (!map) return;
      for (const [key, item] of map) {
        const [setting, value] = key.split(':');
        void item.setChecked(setting === 'theme' ? value === theme : value === density);
      }
    };

    void (async () => {
      try {
        const tauri = await import('@tauri-apps/api/menu');
        if (!live) return;
        const ctx: BuildContext = {
          tauri,
          state: { theme: ref.current.theme, density: ref.current.density },
          run: (command) => {
            // Logged like the keyboard's own commands are, because a menu
            // click leaves no other trace: nothing changes in the DOM to say
            // where an action came from.
            void api.log(JSON.stringify({ kind: 'menu', command }));
            // A switch rather than an if/else chain so the compiler is the
            // thing that notices a new MENU_COMMAND with nowhere to go. The
            // chain this replaced ended in `else openSettings()`, which meant
            // any command it did not know about quietly opened Settings —
            // the exact silent-wrong-action the header claims cannot happen.
            switch (command) {
              case 'new-message':
                return ref.current.newMessage();
              case 'settings':
                return ref.current.openSettings();
              case 'help':
                return ref.current.openHelp();
              case 'find':
                return ref.current.find();
              default:
                return ((x: never) => x)(command);
            }
          },
          choose: (setting, value) => {
            void api.log(JSON.stringify({ kind: 'menu', command: `${setting}:${value}` }));
            if (setting === 'theme') {
              ref.current.setTheme(value as Settings['theme']);
              // Corrected here as well as in the effect below: a check item
              // toggles itself on click, so choosing the value that was
              // already set would otherwise clear its own mark, and the
              // effect will not fire because nothing changed.
              mark(value, ref.current.density);
            } else {
              ref.current.setDensity(value as Settings['density']);
              mark(ref.current.theme, value);
            }
          },
          checks: new Map(),
          windowMenu: null,
          helpMenu: null,
        };
        checks.current = await install(ctx);
      } catch (e) {
        // A window with no menu bar is a poorer window, not a broken one. This
        // must never be the reason the app fails to start.
        void api.log(JSON.stringify({ kind: 'menu-failed', error: String(e) }));
      }
    })();

    return () => {
      live = false;
    };
  }, []);

  // Settings change from the Appearance pane too, and the marks follow.
  useEffect(() => {
    const map = checks.current;
    if (!map) return;
    for (const [key, item] of map) {
      const [setting, value] = key.split(':');
      const want = setting === 'theme' ? bindings.theme : bindings.density;
      void item.setChecked(value === want);
    }
  }, [bindings.theme, bindings.density]);
}
