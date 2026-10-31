/**
 * Recipient lines in the reader: who goes where in a localized role string, and
 * what Copy vs Copy email address should write given a selection and a click.
 *
 * Addresses never come from visible labels — only from `data-mail-address`, paired
 * to labels by index the way the engine sends them.
 */

/** Marks `{ $who }` when formatting a role so `splitRole` can wrap the parties. */
export const ROLE_SLOT = '\uE000';

/** Splits a formatted role on the slot. A broken translation with two slots
 *  still yields one split, so rendering never throws. */
export function splitRole(formatted: string): { before: string; after: string } {
  const i = formatted.indexOf(ROLE_SLOT);
  if (i === -1) return { before: formatted, after: '' };
  return { before: formatted.slice(0, i), after: formatted.slice(i + ROLE_SLOT.length) };
}

export function pairRecipients({ labels, addrs }: { labels: string[]; addrs: string[] }) {
  return labels.map((label, i) => ({ label, addr: addrs[i] ?? '' }));
}

export function addressesToCopy(addrs: string[]): string {
  return addrs.filter((a) => a !== '').join(', ');
}

function partyFromTarget(target: Element | null): Element | null {
  return target?.closest('[data-mail-address]') ?? null;
}

function addressFromParty(party: Element): string {
  return party.getAttribute('data-mail-address') ?? '';
}

function partiesInSelection(parties: Element[], selection: Selection): Element[] {
  return parties.filter((p) => selection.containsNode(p, true));
}

/** Mailbox string for Copy email address, from selection and pointer. */
export function addressesForCopy({
  parties,
  selection,
  target,
}: {
  parties: Element[];
  selection: Selection | null;
  target: Element | null;
}): string {
  const party = partyFromTarget(target);
  if (!party) return '';
  const here = addressFromParty(party);
  if (!selection || selection.isCollapsed) {
    return addressesToCopy([here]);
  }
  const selected = partiesInSelection(parties, selection);
  if (selected.includes(party)) {
    return addressesToCopy(selected.map(addressFromParty));
  }
  return addressesToCopy([here]);
}

const MENU = '[data-recipient-menu]';

/** A press outside the menu dismisses it. A press on an item must reach the item. */
export function pressDismissesMenu(target: { closest(selector: string): unknown } | null): boolean {
  if (!target) return true;
  return target.closest(MENU) == null;
}

/** Focusing the message frame. That document never delivers the click here,
 *  and a system menu would already have closed. */
export function focusDismissesMenu(target: { tagName?: string } | null): boolean {
  return target?.tagName === 'IFRAME';
}

const TOGGLE = '.msg-toggle';

/** Keys that only change another key or a click, and activate nothing alone. */
const MODIFIER_KEYS = new Set(['Shift', 'Control', 'Alt', 'AltGraph', 'Meta', 'CapsLock', 'Fn']);

type Press = { target: unknown; button: number; preventDefault(): void; stopPropagation(): void };
type Click = { target: unknown; preventDefault(): void; stopPropagation(): void };

function onToggle(target: unknown): boolean {
  const el = target as { closest?: (selector: string) => unknown } | null;
  return typeof el?.closest === 'function' && el.closest(TOGGLE) != null;
}

/**
 * The click that belongs to a press dismissing the menu.
 *
 * The names sit inside the message's collapse button, so a press there that
 * dismisses the menu would close the message as well. That press and its click
 * are taken. Anywhere else the press goes on to what it landed on, as it does
 * for the list's menu and inside the message, which is another document.
 *
 * The click arrives after the menu has unmounted, so this outlives the menu,
 * one per window. A press can end without a click, released over the message
 * or outside the window, so the next press or key clears what it left. The
 * eater that waited for "the next click" took whatever came, anywhere,
 * Enter on Send included.
 */
export function dismissClickRule() {
  let pending: unknown = null;
  return {
    /** From the menu: `press` dismissed it. */
    dismissedBy(press: Press) {
      if (press.button !== 0 || !onToggle(press.target)) return;
      press.preventDefault();
      press.stopPropagation();
      pending = press;
    },
    // The press that arms the rule can reach this before or after the menu
    // reports it, so only a different press clears it.
    pointerdown(e: unknown) {
      if (e !== pending) pending = null;
    },
    keydown(e: { key: string }) {
      if (!MODIFIER_KEYS.has(e.key)) pending = null;
    },
    click(e: Click) {
      if (pending === null) return;
      pending = null;
      // Released somewhere else, the click lands on what the two share, and
      // closes nothing.
      if (!onToggle(e.target)) return;
      e.preventDefault();
      e.stopPropagation();
    },
  };
}

let windowRule: ReturnType<typeof dismissClickRule> | null = null;

/** The rule for this window, put in force by the first menu opened in it. */
export function dismissClicks() {
  if (!windowRule) {
    const rule = dismissClickRule();
    window.addEventListener('pointerdown', rule.pointerdown, true);
    window.addEventListener('keydown', rule.keydown, true);
    window.addEventListener('click', rule.click, true);
    windowRule = rule;
  }
  return windowRule;
}

/** Visible text for Copy when the selection does not govern the gesture. */
export function recipientCopyText({
  selection,
  target,
  partyLabel,
  parties,
}: {
  selection: Selection | null;
  target: Element | null;
  partyLabel: string;
  parties: Element[];
}): string {
  const party = partyFromTarget(target);
  if (!selection || selection.isCollapsed || !party) {
    return partyLabel;
  }
  const selected = partiesInSelection(parties, selection);
  if (selected.includes(party)) {
    // A selection with no text is not a copy of the name. The label is.
    const text = selection.toString();
    return text || partyLabel;
  }
  return partyLabel;
}

type ClipboardDeps = {
  getClipboard?: () => Clipboard | undefined;
  writeText?: (text: string) => Promise<void>;
  execCommand?: (command: string) => boolean;
  document?: Document;
};

/** Writes plain text, falling back when the async clipboard API is missing or denied. */
export async function writeClipboard(text: string, deps: ClipboardDeps = {}): Promise<void> {
  const doc = deps.document ?? document;
  const clipboard = deps.getClipboard?.() ?? navigator.clipboard;
  const writeText =
    deps.writeText ??
    ((value: string) => {
      if (!clipboard?.writeText) return Promise.reject(new Error('no clipboard'));
      return clipboard.writeText(value);
    });

  // The fallback has to run in this turn. A clipboard rejection arrives after
  // the click's user activation is gone, and execCommand would then copy
  // nothing — leaving Command-C to copy the selected name instead.
  const copyNow = () => {
    const area = doc.createElement('textarea');
    area.value = text;
    area.setAttribute('readonly', '');
    area.setAttribute('aria-hidden', 'true');
    area.tabIndex = -1;
    area.style.position = 'fixed';
    area.style.opacity = '0';
    area.style.insetInlineStart = '0';
    area.style.insetBlockStart = '0';
    area.style.inlineSize = '1px';
    area.style.blockSize = '1px';
    const exec = deps.execCommand ?? ((cmd) => doc.execCommand(cmd));
    try {
      doc.body.appendChild(area);
      area.select();
      exec('copy');
    } finally {
      area.remove();
    }
  };

  // Swallow the rejection here. copyNow runs before this promise settles, and
  // an unhandled rejection would otherwise surface if the synchronous copy throws.
  let pending: Promise<void>;
  try {
    pending = writeText(text).then(
      () => undefined,
      () => undefined,
    );
  } catch {
    copyNow();
    return;
  }
  copyNow();
  await pending;
}
