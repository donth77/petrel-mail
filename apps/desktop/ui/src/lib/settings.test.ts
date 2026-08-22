import { describe, expect, it } from 'vitest';
import { clampRail, DEFAULTS, RAIL_MAX, RAIL_MIN } from './settings';

/**
 * The rail width comes from persisted text and feeds a CSS length. Both ends of
 * that trip can produce something unusable, and the failure is unrecoverable
 * from inside the app: a rail dragged to nothing has no handle left to drag
 * back, and a NaN width collapses the pane with no way to reopen it.
 */
describe('clampRail', () => {
  it('keeps sensible widths untouched', () => {
    expect(clampRail(236)).toBe(236);
    expect(clampRail('300')).toBe(300);
  });

  it('holds the range at both ends', () => {
    expect(clampRail(0)).toBe(RAIL_MIN);
    expect(clampRail(-500)).toBe(RAIL_MIN);
    expect(clampRail(99_999)).toBe(RAIL_MAX);
  });

  it('falls back to the default rather than producing NaN', () => {
    // A hand-edited or half-written settings row must not be able to make the
    // sidebar disappear with no way back.
    for (const bad of ['', 'wide', 'NaN', 'undefined']) {
      expect(clampRail(bad)).toBe(Number(DEFAULTS.railWidth));
    }
  });

  it('rounds, because a fractional CSS pixel is a blurry border', () => {
    expect(clampRail(240.6)).toBe(241);
  });

  it('has a default inside its own range', () => {
    const d = Number(DEFAULTS.railWidth);
    expect(d).toBeGreaterThanOrEqual(RAIL_MIN);
    expect(d).toBeLessThanOrEqual(RAIL_MAX);
  });
});
