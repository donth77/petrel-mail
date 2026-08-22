import { t } from './strings';
import type { PickerOption } from '../components/Picker';

/**
 * The snooze presets, resolved against the current time (docs/design Pickers).
 *
 * Each option carries the instant it resolves to as its id, and shows that
 * instant beside its name: "Tomorrow" is not a time, and a picker that hides
 * which one it means is a picker you have to test on a real message to trust.
 *
 * The hours are deliberate rather than round-number arithmetic. "Later today"
 * is three hours on, but never past the evening — a message put aside at 6pm
 * should not return at 9pm. "Tomorrow" is the start of a working day, not
 * twenty-four hours from whenever you happened to press the key.
 */
export function snoozeOptions(now: Date = new Date()): PickerOption[] {
  const at = (d: Date, hour: number) => {
    const x = new Date(d);
    x.setHours(hour, 0, 0, 0);
    return x;
  };
  const addDays = (d: Date, n: number) => {
    const x = new Date(d);
    x.setDate(x.getDate() + n);
    return x;
  };

  // Three hours on, but clamped into waking hours at both ends. Past 8pm it
  // becomes tomorrow morning; before 8am it becomes this morning. Without the
  // lower bound, snoozing at 1am offers "later today — 4:03 AM", which is three
  // hours on and no use to anyone.
  const laterToday = new Date(now.getTime() + 3 * 60 * 60 * 1000);
  const later =
    laterToday > at(now, 20)
      ? at(addDays(now, 1), 8)
      : laterToday < at(now, 8)
        ? at(now, 8)
        : laterToday;

  const tomorrow = at(addDays(now, 1), 8);

  // Saturday morning. If it is already the weekend, the useful answer is the
  // one still ahead of you — Saturday morning if that has not passed, otherwise
  // next Saturday rather than a time in the past.
  const saturdayThisWeek = at(addDays(now, (6 - now.getDay() + 7) % 7), 9);
  const weekend =
    saturdayThisWeek > now ? saturdayThisWeek : at(addDays(saturdayThisWeek, 7), 9);

  // Monday morning.
  const daysToMonday = (1 - now.getDay() + 7) % 7 || 7;
  const nextWeek = at(addDays(now, daysToMonday), 8);

  // A bare weekday is ambiguous the moment a date is more than a week out:
  // "Sat 9:00 AM" on a Saturday reads as today when it means next Saturday.
  // Anything past tomorrow carries its date.
  const sameDay = (a: Date, b: Date) => a.toDateString() === b.toDateString();
  const label = (d: Date) => {
    const time = d.toLocaleString(undefined, { hour: 'numeric', minute: '2-digit' });
    if (sameDay(d, now)) return time;
    if (sameDay(d, addDays(now, 1))) return `${t('snooze-tomorrow')} ${time}`;
    const near = d.getTime() - now.getTime() < 6 * 24 * 60 * 60 * 1000;
    return `${d.toLocaleDateString(undefined, near ? { weekday: 'short' } : { weekday: 'short', day: 'numeric', month: 'short' })} ${time}`;
  };

  return [
    { id: later.getTime(), label: t('snooze-later'), detail: label(later) },
    { id: tomorrow.getTime(), label: t('snooze-tomorrow'), detail: label(tomorrow) },
    { id: weekend.getTime(), label: t('snooze-weekend'), detail: label(weekend) },
    { id: nextWeek.getTime(), label: t('snooze-next-week'), detail: label(nextWeek) },
  ];
}
