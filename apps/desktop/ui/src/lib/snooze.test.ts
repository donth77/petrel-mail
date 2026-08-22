import { describe, expect, it } from 'vitest';
import { snoozeOptions } from './snooze';

/**
 * Snooze presets resolve against the clock, which makes them exactly the kind
 * of code that looks right and is wrong at 1am on a Saturday. Both bugs pinned
 * below were found by eye, once each, in the running app — which is not a
 * process that scales.
 *
 * Assertions are on the resolved Date, not on formatted text: the labels go
 * through toLocaleString and would tie these tests to a locale and a timezone
 * rather than to the behaviour.
 */

const at = (iso: string) => new Date(iso);
const opt = (now: Date, label: string) => {
  const found = snoozeOptions(now).find((o) => o.label.toLowerCase().includes(label));
  if (!found) throw new Error(`no option matching ${label}`);
  return { ...found, when: new Date(found.id) };
};

describe('later today', () => {
  it('is three hours on during the working day', () => {
    const now = at('2026-08-19T10:00:00');
    expect(opt(now, 'later').when.getHours()).toBe(13);
  });

  it('does not land in the small hours when set overnight', () => {
    // The bug: at 01:04 this offered "later today — 4:03 AM".
    const now = at('2026-08-22T01:04:00');
    const when = opt(now, 'later').when;
    expect(when.getHours()).toBe(8);
    expect(when.getDate()).toBe(22);
  });

  it('rolls to tomorrow morning when set late in the evening', () => {
    const now = at('2026-08-19T22:30:00');
    const when = opt(now, 'later').when;
    expect(when.getDate()).toBe(20);
    expect(when.getHours()).toBe(8);
  });

  it('is always in the future', () => {
    for (const hour of [0, 3, 7, 9, 13, 17, 19, 20, 21, 23]) {
      const now = at(`2026-08-19T${String(hour).padStart(2, '0')}:30:00`);
      expect(opt(now, 'later').when.getTime()).toBeGreaterThan(now.getTime());
    }
  });
});

describe('tomorrow', () => {
  it('is the start of the next day, not twenty-four hours on', () => {
    const now = at('2026-08-19T22:30:00');
    const when = opt(now, 'tomorrow').when;
    expect(when.getDate()).toBe(20);
    expect(when.getHours()).toBe(8);
  });
});

describe('this weekend', () => {
  it('is the coming Saturday from midweek', () => {
    // Wednesday 19 August 2026.
    const now = at('2026-08-19T10:00:00');
    const when = opt(now, 'weekend').when;
    expect(when.getDay()).toBe(6);
    expect(when.getDate()).toBe(22);
    expect(when.getHours()).toBe(9);
  });

  it('means later today when Saturday morning has not happened yet', () => {
    const now = at('2026-08-22T01:04:00');
    const when = opt(now, 'weekend').when;
    expect(when.getDate()).toBe(22);
    expect(when.getHours()).toBe(9);
  });

  it('rolls to next Saturday once this one has passed', () => {
    const now = at('2026-08-22T14:00:00');
    const when = opt(now, 'weekend').when;
    expect(when.getDay()).toBe(6);
    expect(when.getDate()).toBe(29);
  });
});

describe('next week', () => {
  it('is Monday morning', () => {
    const now = at('2026-08-19T10:00:00');
    const when = opt(now, 'next week').when;
    expect(when.getDay()).toBe(1);
    expect(when.getHours()).toBe(8);
  });

  it('is next Monday, not today, when set on a Monday', () => {
    const now = at('2026-08-24T10:00:00');
    const when = opt(now, 'next week').when;
    expect(when.getDay()).toBe(1);
    expect(when.getDate()).toBe(31);
  });
});

describe('every option', () => {
  it('is in the future, at every hour of every weekday', () => {
    for (let day = 17; day <= 23; day++) {
      for (const hour of [0, 6, 9, 12, 18, 21, 23]) {
        const now = at(`2026-08-${day}T${String(hour).padStart(2, '0')}:15:00`);
        for (const o of snoozeOptions(now)) {
          expect(
            new Date(o.id).getTime(),
            `${o.label} on Aug ${day} at ${hour}:15 resolved to the past`,
          ).toBeGreaterThan(now.getTime());
        }
      }
    }
  });

  it('shows the instant it resolves to, so the name is never the only clue', () => {
    for (const o of snoozeOptions(at('2026-08-19T10:00:00'))) {
      expect(o.detail, `${o.label} has no resolved time`).toBeTruthy();
    }
  });

  it('disambiguates a weekday more than a week out with its date', () => {
    // Saturday afternoon: "this weekend" is next Saturday and must not read
    // as today.
    const now = at('2026-08-22T14:00:00');
    const detail = opt(now, 'weekend').detail ?? '';
    expect(detail).toMatch(/29|Aug/);
  });
});
