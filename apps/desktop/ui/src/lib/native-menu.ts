/**
 * WebKit's own context menu, kept only where it has something to offer.
 *
 * Over the app's own controls it offered one item, Reload, which reads like
 * "check for mail" and restarts the window: an open composer was gone, and with
 * it whatever had been typed since the last autosave pause. So the native menu
 * is left to the two places it means something. A field keeps its editing and
 * spelling items. Selected text keeps Look Up, Translate and Copy, and since a
 * context click on a word selects it before the menu is asked for, a name in
 * the sidebar or a subject line keeps them too. Everywhere else a context click
 * does nothing, unless Petrel has a menu of its own there, as a list row does.
 *
 * Message bodies are documents of their own, in frames, and this never sees a
 * click inside one.
 */

/** Inputs that are typed into, whose menu is Cut, Copy, Paste and spelling. A
 *  checkbox or a slider is an input too, and its menu is the lone Reload. */
const TYPED_INPUTS = new Set(['text', 'search', 'email', 'url', 'tel', 'password', 'number']);

/** Whether a context click on `target` gets WebKit's own menu. */
export function keepsNativeMenu(target: Element | null, selection: Selection | null): boolean {
  if (!target) return false;
  if ((target as HTMLElement).isContentEditable) return true;
  const field = target.closest('input, textarea');
  if (field?.tagName === 'TEXTAREA') return true;
  if (field?.tagName === 'INPUT') return TYPED_INPUTS.has((field as HTMLInputElement).type);
  // A named recipient has Petrel's own menu. This has to precede the selection
  // check: a context click selects the word first, so the name is selected by
  // the time the menu is asked for, and WebKit's Copy would write that label.
  // The name sits inside the collapse button. Refusing the menu here is what
  // eats the Control-click that follows; a preventDefault in the button does
  // not, and the message would close.
  if (target.closest('[data-mail-address]')) return false;
  // On the selection itself, not merely while something is selected: text
  // selected in the reader says nothing about a click on the Compose button.
  return !!selection && !selection.isCollapsed && selection.containsNode(target, true);
}

type Click = Pick<MouseEvent, 'target' | 'button' | 'ctrlKey' | 'preventDefault' | 'stopPropagation'>;

/**
 * The listeners that hold the rule, apart from installing them so a test can
 * drive them.
 *
 * A Control-click is the Mac's context click, and when no menu takes its
 * release WebKit sends a click as well: with the menu refused and nothing
 * more, a Control-click on a sidebar folder opened it, and on the reader's
 * Archive button archived. So a Control-click whose menu is refused does
 * nothing else either, as a two-finger click never does. The next press
 * clears that, so it can only ever eat the click that belongs to it.
 */
export function nativeMenuRule(selection: () => Selection | null) {
  let contextClick = false;
  return {
    pointerdown: () => {
      contextClick = false;
    },
    contextmenu: (e: Click) => {
      const target = e.target instanceof Element ? e.target : null;
      if (keepsNativeMenu(target, selection())) return;
      e.preventDefault();
      contextClick = e.button === 0 && e.ctrlKey;
    },
    click: (e: Click) => {
      if (!contextClick || !e.ctrlKey) return;
      contextClick = false;
      e.preventDefault();
      e.stopPropagation();
    },
  };
}

/** Puts the rule in force for this window. Once, at startup. */
export function limitNativeMenu() {
  const rule = nativeMenuRule(() => window.getSelection());
  // Capturing, so the decision is made before anything in the page answers
  // the click. Petrel's own menus still open: preventing WebKit's menu does
  // not stop the event reaching them.
  window.addEventListener('pointerdown', rule.pointerdown, true);
  window.addEventListener('contextmenu', rule.contextmenu, true);
  window.addEventListener('click', rule.click, true);
}
