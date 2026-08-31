import { describe, expect, it } from 'vitest';
import { BINDINGS } from './shortcuts';

/**
 * The keyboard map is meant to be the one place a binding is declared, so that
 * a shortcut cannot exist without being documented. That only holds if nothing
 * quietly claims a key twice — the failure there is silent and specific: two
 * handlers on one key, the first one wins, and the second is a feature that
 * simply does nothing for the person who read about it in Help.
 */
describe('the keyboard map', () => {
  const available = BINDINGS.filter((s) => s.available);

  const signature = (c: { key: string; shift?: boolean; meta?: boolean; then?: string }) =>
    [c.meta ? 'meta' : '', c.shift ? 'shift' : '', c.key, c.then ?? ''].filter(Boolean).join('+');

  it('never gives one chord to two actions', () => {
    const seen = new Map<string, string>();
    for (const binding of available) {
      for (const chord of binding.chords) {
        const sig = signature(chord);
        const owner = seen.get(sig);
        expect(owner, `${sig} is claimed by both ${owner} and ${binding.id}`).toBeUndefined();
        seen.set(sig, binding.id);
      }
    }
  });

  it('keeps plain and shifted forms of a letter apart', () => {
    // I moves to the inbox, ⇧I marks read; U goes back to the list, ⇧U marks
    // unread. Those pairs are only safe because the modifier is part of what
    // identifies a chord — if it stopped being, each pair would collapse onto
    // one action and the other would vanish.
    const plainI = available.find((s) => s.chords.some((c) => c.key === 'i' && !c.shift));
    const shiftI = available.find((s) => s.chords.some((c) => c.key === 'i' && c.shift));
    expect(plainI?.id).toBe('move-inbox');
    expect(shiftI?.id).toBe('read-unread');
    expect(plainI).not.toBe(shiftI);
  });

  it('carries the two that were added last, so they cannot be dropped quietly', () => {
    const inbox = available.find((s) => s.id === 'move-inbox');
    const popOut = available.find((s) => s.id === 'pop-out');
    expect(inbox?.chords).toEqual([{ key: 'i' }]);
    expect(popOut?.chords).toEqual([{ key: 'o' }]);
    // Help renders from this list, so a binding without a label is a row with
    // a key and no explanation.
    expect(inbox?.label).toBeTruthy();
    expect(popOut?.label).toBeTruthy();
  });

  it('gives every available binding at least one chord and a label', () => {
    for (const binding of available) {
      expect(binding.chords.length, `${binding.id} has no chord`).toBeGreaterThan(0);
      expect(binding.label, `${binding.id} has no label`).toBeTruthy();
    }
  });
});
