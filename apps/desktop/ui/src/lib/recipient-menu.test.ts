import { afterEach, describe, expect, it, vi } from 'vitest';
import { availableLocales, setLocale, t } from './strings';
import {
  ROLE_SLOT,
  addressesForCopy,
  addressesToCopy,
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
