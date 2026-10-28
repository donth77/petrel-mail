import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { keepsNativeMenu, nativeMenuRule } from './native-menu';

/* Where WebKit's own context menu is kept. Everywhere else it was a lone
   Reload, which restarted the window and took an open composer with it.

   There is no document here, so elements are stood in for by what the rule
   reads of them: whether they are editable, what `closest` finds, and whether
   they are inside the selection. */

class FakeElement {
  constructor(
    readonly tagName: string,
    readonly opts: {
      type?: string;
      editable?: boolean;
      field?: FakeElement;
      mailAddress?: string;
      mailParty?: FakeElement;
    } = {},
  ) {}
  get type() {
    return this.opts.type ?? '';
  }
  get isContentEditable() {
    return this.opts.editable ?? false;
  }
  closest(selector: string) {
    if (selector === '[data-mail-address]') {
      if (this.opts.mailAddress !== undefined) return this;
      return this.opts.mailParty ?? null;
    }
    if (selector !== 'input, textarea') return null;
    if (this.tagName === 'INPUT' || this.tagName === 'TEXTAREA') return this;
    return this.opts.field ?? null;
  }
}
const el = (tagName: string, opts: FakeElement['opts'] = {}) => new FakeElement(tagName, opts) as unknown as Element;

/** A selection, with the elements it covers. */
const selecting = (...covered: Element[]) =>
  ({
    isCollapsed: covered.length === 0,
    containsNode: (node: Node) => covered.includes(node as unknown as Element),
  }) as unknown as Selection;

beforeEach(() => vi.stubGlobal('Element', FakeElement));
afterEach(() => vi.unstubAllGlobals());

describe('keepsNativeMenu', () => {
  it('refuses it on the controls, where it was a lone Reload', () => {
    for (const target of [el('BUTTON'), el('DIV'), el('SPAN'), el('INPUT', { type: 'checkbox' }), el('INPUT', { type: 'range' })]) {
      expect(keepsNativeMenu(target, selecting())).toBe(false);
    }
    expect(keepsNativeMenu(null, selecting())).toBe(false);
  });

  it('keeps it in a field, for cut, copy, paste and spelling', () => {
    for (const type of ['text', 'search', 'email', 'password']) {
      expect(keepsNativeMenu(el('INPUT', { type }), selecting()), type).toBe(true);
    }
    expect(keepsNativeMenu(el('TEXTAREA'), selecting())).toBe(true);
    // The message body being written, wherever in it the click lands.
    expect(keepsNativeMenu(el('P', { editable: true }), selecting())).toBe(true);
  });

  it('keeps it on selected text, for Look Up and Copy', () => {
    const subject = el('H2');
    expect(keepsNativeMenu(subject, selecting(subject))).toBe(true);
  });

  it('refuses it on a control while text elsewhere is selected', () => {
    const subject = el('H2');
    expect(keepsNativeMenu(el('BUTTON'), selecting(subject))).toBe(false);
  });

  it('refuses it on a named recipient even when the selection contains them', () => {
    const party = new FakeElement('SPAN', { mailAddress: 'a@example.com' });
    const inner = new FakeElement('SPAN', { mailParty: party });
    const partyEl = party as unknown as Element;
    const innerEl = inner as unknown as Element;
    expect(keepsNativeMenu(partyEl, selecting(partyEl))).toBe(false);
    expect(keepsNativeMenu(innerEl, selecting(partyEl))).toBe(false);
  });
});

describe('a Control-click whose menu is refused', () => {
  type Seen = { target: Element; button: number; ctrlKey: boolean; prevented: boolean; stopped: boolean };
  const event = (target: Element, button: number, ctrlKey: boolean) => {
    const e: Seen & { preventDefault(): void; stopPropagation(): void } = {
      target,
      button,
      ctrlKey,
      prevented: false,
      stopped: false,
      preventDefault() {
        e.prevented = true;
      },
      stopPropagation() {
        e.stopped = true;
      },
    };
    return e;
  };

  it('does not go on to click the button under it', () => {
    // WebKit sends the click once no menu takes the release: the reader's
    // Archive button archived.
    const rule = nativeMenuRule(() => selecting());
    const archive = el('BUTTON');
    rule.pointerdown();
    const menu = event(archive, 0, true);
    rule.contextmenu(menu);
    const click = event(archive, 0, true);
    rule.click(click);
    expect(menu.prevented).toBe(true);
    expect([click.prevented, click.stopped]).toEqual([true, true]);
  });

  it('refuses and swallows a Control-click on a named recipient', () => {
    // The word is already selected, which is when WebKit would otherwise keep
    // its own Copy. The refusal still has to swallow the click.
    const party = el('SPAN', { mailAddress: 'a@example.com' });
    const rule = nativeMenuRule(() => selecting(party));
    rule.pointerdown();
    const menu = event(party, 0, true);
    rule.contextmenu(menu);
    const click = event(party, 0, true);
    rule.click(click);
    expect(menu.prevented).toBe(true);
    expect([click.prevented, click.stopped]).toEqual([true, true]);
  });

  it('leaves the next click alone', () => {
    const rule = nativeMenuRule(() => selecting());
    const archive = el('BUTTON');
    rule.pointerdown();
    rule.contextmenu(event(archive, 0, true));
    // Chromium sends no click after a Control-click, so the next press must
    // clear what this one left, even a press with Control held.
    rule.pointerdown();
    const next = event(archive, 0, true);
    rule.click(next);
    expect([next.prevented, next.stopped]).toEqual([false, false]);
  });

  it('touches nothing after a two-finger click, or where the menu is kept', () => {
    const rule = nativeMenuRule(() => selecting());
    rule.pointerdown();
    rule.contextmenu(event(el('BUTTON'), 2, false));
    const click = event(el('BUTTON'), 0, true);
    rule.click(click);
    expect(click.stopped).toBe(false);

    const field = el('INPUT', { type: 'text' });
    rule.pointerdown();
    const menu = event(field, 0, true);
    rule.contextmenu(menu);
    expect(menu.prevented).toBe(false);
    const fieldClick = event(field, 0, true);
    rule.click(fieldClick);
    expect(fieldClick.stopped).toBe(false);
  });
});
