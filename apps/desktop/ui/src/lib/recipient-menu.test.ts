import { afterEach, describe, expect, it, vi } from 'vitest';
import { availableLocales, setLocale, t } from './strings';
import {
  ROLE_SLOT,
  addressesForCopy,
  addressesToCopy,
  dismissClickRule,
  focusDismissesMenu,
  pairRecipients,
  pressDismissesMenu,
  recipientCopyText,
  splitRole,
  writeClipboard,
} from './recipient-menu';

class FakeParty {
  constructor(
    readonly mailAddress: string,
    readonly label: string,
    readonly opts: { mailParty?: FakeParty } = {},
  ) {}
  closest(selector: string) {
    if (selector === '[data-mail-address]') {
      if (this.opts.mailParty) return this.opts.mailParty;
      return this;
    }
    return null;
  }
  getAttribute(name: string) {
    if (name === 'data-mail-address') return this.mailAddress;
    return null;
  }
}
const party = (mailAddress: string, label: string, opts: FakeParty['opts'] = {}) =>
  new FakeParty(mailAddress, label, opts) as unknown as Element;

const selecting = (text: string, ...covered: Element[]) =>
  ({
    isCollapsed: covered.length === 0,
    containsNode: (node: Node) => covered.includes(node as unknown as Element),
    toString: () => text,
  }) as unknown as Selection;

describe('pairRecipients and addressesToCopy', () => {
  it('zips by index and drops empty addresses', () => {
    expect(pairRecipients({ labels: ['A', 'B', 'C'], addrs: ['a@x.com'] })).toEqual([
      { label: 'A', addr: 'a@x.com' },
      { label: 'B', addr: '' },
      { label: 'C', addr: '' },
    ]);
    expect(addressesToCopy(['a@x.com', '', 'b@x.com'])).toBe('a@x.com, b@x.com');
  });
});

describe('splitRole', () => {
  it('splits on the first slot only', () => {
    expect(splitRole(`before${ROLE_SLOT}after`)).toEqual({ before: 'before', after: 'after' });
    expect(splitRole(`x${ROLE_SLOT}y${ROLE_SLOT}z`)).toEqual({ before: 'x', after: `y${ROLE_SLOT}z` });
    expect(splitRole('whole')).toEqual({ before: 'whole', after: '' });
  });
});

describe('addressesForCopy', () => {
  const a = party('a@example.com', 'A');
  const b = party('b@example.com', 'B');
  const c = party('c@example.com', 'C');
  const parties = [a, b, c];

  it('joins selected parties when the pointer is inside the selection', () => {
    const sel = selecting('A, B', a, b);
    expect(addressesForCopy({ parties, selection: sel, target: a })).toBe('a@example.com, b@example.com');
  });

  it('copies only the party under the pointer when it is outside the selection', () => {
    const sel = selecting('A, B', a, b);
    expect(addressesForCopy({ parties, selection: sel, target: c })).toBe('c@example.com');
  });

  it('reads the attribute, not the visible label', () => {
    const labeled = party('dana@example.com', 'Dana Wu');
    expect(
      addressesForCopy({
        parties: [labeled],
        selection: selecting('', labeled),
        target: labeled,
      }),
    ).toBe('dana@example.com');
  });

  it('copies the target party when the selection is collapsed', () => {
    expect(addressesForCopy({ parties, selection: selecting(''), target: b })).toBe('b@example.com');
  });
});

describe('recipientCopyText', () => {
  const a = party('a@example.com', 'Alice');
  const b = party('b@example.com', 'Bob');
  const parties = [a, b];

  it('uses the selection when it touches the target party', () => {
    const sel = selecting('Alice, Bob', a, b);
    expect(recipientCopyText({ selection: sel, target: a, partyLabel: 'Alice', parties })).toBe('Alice, Bob');
  });

  it('uses the label when the selection has no text', () => {
    const sel = selecting('', a);
    expect(recipientCopyText({ selection: sel, target: a, partyLabel: 'Alice', parties })).toBe('Alice');
  });

  it('uses the party label when the selection is elsewhere', () => {
    const sel = selecting('Alice', a);
    expect(recipientCopyText({ selection: sel, target: b, partyLabel: 'Bob', parties })).toBe('Bob');
  });
});

describe('dismissing the menu', () => {
  const inside = { closest: (selector: string) => (selector === '[data-recipient-menu]' ? inside : null) };
  const outside = { closest: () => null };

  it('lets a press on an item through, and treats every other press as dismiss', () => {
    expect(pressDismissesMenu(inside)).toBe(false);
    expect(pressDismissesMenu(outside)).toBe(true);
    expect(pressDismissesMenu(null)).toBe(true);
  });

  it('treats focus moving into the message frame as dismiss', () => {
    expect(focusDismissesMenu({ tagName: 'IFRAME' })).toBe(true);
    expect(focusDismissesMenu({ tagName: 'BUTTON' })).toBe(false);
    expect(focusDismissesMenu(null)).toBe(false);
  });
});

describe('the click after a press that dismisses the menu', () => {
  /* What the rule reads of an element: whether it sits inside a message's
     collapse button, where the names are. */
  const toggle = { closest: (selector: string) => (selector === '.msg-toggle' ? toggle : null) };
  const archive = { closest: () => null };

  type Seen = { target: unknown; button: number; prevented: boolean; stopped: boolean };
  const event = (target: unknown, button = 0) => {
    const e: Seen & { preventDefault(): void; stopPropagation(): void } = {
      target,
      button,
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

  /** A press the menu reports as its dismissal, seen by the window first. */
  const dismissWith = (rule: ReturnType<typeof dismissClickRule>, press: ReturnType<typeof event>) => {
    rule.pointerdown(press);
    rule.dismissedBy(press);
  };

  it('takes a press on the collapse button and its click, which would close the message', () => {
    const rule = dismissClickRule();
    const press = event(toggle);
    dismissWith(rule, press);
    const click = event(toggle);
    rule.click(click);
    expect([press.prevented, press.stopped]).toEqual([true, true]);
    expect([click.prevented, click.stopped]).toEqual([true, true]);
  });

  it('takes it whichever listener hears the press first', () => {
    const rule = dismissClickRule();
    const press = event(toggle);
    rule.dismissedBy(press);
    rule.pointerdown(press);
    const click = event(toggle);
    rule.click(click);
    expect(click.prevented).toBe(true);
  });

  it('leaves the next click alone when the press ended without one', () => {
    // Released over the message, or outside the window, the press brings no
    // click here. The eater waited, and took the next click wherever it came.
    const rule = dismissClickRule();
    dismissWith(rule, event(toggle));
    rule.pointerdown(event(toggle));
    const click = event(toggle);
    rule.click(click);
    expect([click.prevented, click.stopped]).toEqual([false, false]);
  });

  it('leaves a click from the keyboard alone', () => {
    const rule = dismissClickRule();
    dismissWith(rule, event(toggle));
    rule.keydown({ key: 'Enter' });
    const click = event(toggle);
    rule.click(click);
    expect([click.prevented, click.stopped]).toEqual([false, false]);
  });

  it('still takes the click when a modifier goes down during the press', () => {
    // Windows repeats a held Shift as it is held.
    const rule = dismissClickRule();
    dismissWith(rule, event(toggle));
    rule.keydown({ key: 'Shift' });
    const click = event(toggle);
    rule.click(click);
    expect(click.prevented).toBe(true);
  });

  it('never takes a click anywhere but the collapse button', () => {
    // A press that ended elsewhere and a click with no press of its own, as
    // VoiceOver sends: neither is the press's click on the button.
    const rule = dismissClickRule();
    dismissWith(rule, event(toggle));
    const stray = event(archive);
    rule.click(stray);
    expect([stray.prevented, stray.stopped]).toEqual([false, false]);
    const later = event(toggle);
    rule.click(later);
    expect([later.prevented, later.stopped]).toEqual([false, false]);
  });

  it('lets a press elsewhere go on, with its click', () => {
    // As the list's menu does: the press that closes it also archives.
    const rule = dismissClickRule();
    const press = event(archive);
    dismissWith(rule, press);
    const click = event(archive);
    rule.click(click);
    expect([press.prevented, press.stopped, click.prevented, click.stopped]).toEqual([
      false,
      false,
      false,
      false,
    ]);
  });

  it('lets a right-click reach the name under it', () => {
    const rule = dismissClickRule();
    const press = event(toggle, 2);
    dismissWith(rule, press);
    const click = event(toggle);
    rule.click(click);
    expect([press.prevented, press.stopped, click.prevented]).toEqual([false, false, false]);
  });
});

describe('role strings and ROLE_SLOT', () => {
  afterEach(() => setLocale('en'));

  it('place the slot exactly once in every locale', () => {
    for (const locale of availableLocales()) {
      setLocale(locale);
      for (const id of ['reader-to', 'reader-cc', 'reader-bcc'] as const) {
        const formatted = t(id, { who: ROLE_SLOT });
        const count = [...formatted].filter((ch) => ch === ROLE_SLOT).length;
        expect(count, `${locale} ${id}`).toBe(1);
      }
    }
  });
});

describe('writeClipboard', () => {
  it('falls back when writeText rejects', async () => {
    const execCommand = vi.fn(() => true);
    const appendChild = vi.fn();
    const remove = vi.fn();
    const select = vi.fn();
    const area = {
      value: '',
      select,
      remove,
      setAttribute: vi.fn(),
      style: {},
    };
    const doc = {
      createElement: () => area,
      body: { appendChild },
      execCommand,
    } as unknown as Document;

    await writeClipboard('a@example.com', {
      writeText: () => Promise.reject(new Error('denied')),
      execCommand,
      document: doc,
    });

    expect(area.value).toBe('a@example.com');
    expect(execCommand).toHaveBeenCalledWith('copy');
    expect(remove).toHaveBeenCalled();
  });
});
