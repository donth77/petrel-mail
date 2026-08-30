import { ChevronDown, ChevronUp } from 'lucide-react';
import { useSettings } from '../../lib/settings';
import {
  ESSENTIAL,
  arrangementFor,
  countFor,
  MAILBOX_LOOK,
  serialiseArrangement,
  type Arrangement,
  type CountMode,
  type MailboxKey,
} from '../../lib/mailboxes';
import { Icon } from '../Icon';
import { Pill } from './Pill';
import { t, type StringId } from '../../lib/strings';

const MODES: { value: CountMode; label: StringId }[] = [
  { value: 'off', label: 'count-none' },
  { value: 'unread', label: 'count-unread' },
  { value: 'total', label: 'count-all' },
];

/**
 * Which mailboxes the sidebar shows, in what order, and what number each one
 * carries.
 *
 * This replaces a single switch that offered Unread, Everything or None for
 * every row at once — and then listed the mailboxes it did not apply to in its
 * own help text. The exceptions were right; a global label promising otherwise
 * was the part that was wrong. Each row answers for itself here, so there is
 * nothing left to contradict.
 *
 * Showing and hiding is the part with no correct answer, which is what makes
 * it a setting rather than a default: whether the Snoozed row is worth its
 * place is not something the app can know about somebody who has never
 * snoozed anything.
 */
export function Sidebar() {
  const { settings, set } = useSettings();
  const arrangement = arrangementFor(settings.railMailboxes, settings.badges);

  const save = (next: Arrangement) => set('railMailboxes', serialiseArrangement(next));

  const move = (key: MailboxKey, by: -1 | 1) => {
    const order = [...arrangement.order];
    const at = order.indexOf(key);
    const to = at + by;
    if (at < 0 || to < 0 || to >= order.length) return;
    [order[at], order[to]] = [order[to], order[at]];
    save({ ...arrangement, order });
  };

  const toggle = (key: MailboxKey) => {
    const hidden = arrangement.hidden.includes(key)
      ? arrangement.hidden.filter((k) => k !== key)
      : [...arrangement.hidden, key];
    save({ ...arrangement, hidden });
  };

  const setCount = (key: string, mode: CountMode) =>
    save({ ...arrangement, counts: { ...arrangement.counts, [key]: mode } });

  return (
    <div className="pane-body">
      <h1 className="pane-title">{t('settings-sidebar')}</h1>

      <section className="field">
        <div className="flabel">{t('sidebar-mailboxes')}</div>

        {/* Roles written out. A list styled with `list-style: none` loses its
            list semantics in WebKit — which is the engine this app actually
            runs on — so "item 3 of 9" goes with it unless the roles are
            stated. Cheap insurance for a nine-row list somebody may be
            reordering entirely by keyboard. */}
        <ul className="mailbox-rows" role="list">
          {/* A heading over the count controls rather than a paragraph above
              the list. Inside the list and sharing its grid, because two grids
              with the same track sizes still measure their own contents: the
              word sat 178px away from the column it names. The checkbox column
              needs no heading — a checkbox beside a mailbox name says what it
              does. */}
          <li className="mailbox-head" role="presentation" aria-hidden="true">
            <span />
            <span />
            <span className="mailbox-col">{t('sidebar-count-column')}</span>
          </li>
          {arrangement.order.map((key, i) => {
            const hidden = arrangement.hidden.includes(key);
            const essential = key === ESSENTIAL;
            return (
              <li className="mailbox-row" role="listitem" key={key}>
                {/* First in the row, before the name. Reordering is a
                    question about position, and position is read down the left
                    edge — with the controls out on the right the eye had to
                    cross the row to see what it had just moved.

                    Buttons rather than a drag handle: reordering nine rows is
                    a thing done once, it has to work from the keyboard, and a
                    drag list is a great deal of machinery for that. */}
                <div className="mailbox-move">
                  <button
                    type="button"
                    className="move-btn"
                    disabled={i === 0}
                    aria-label={t('sidebar-move-up', { name: t(MAILBOX_LOOK[key].label) })}
                    onClick={() => move(key, -1)}
                  >
                    <Icon icon={ChevronUp} size={14} />
                  </button>
                  <button
                    type="button"
                    className="move-btn"
                    disabled={i === arrangement.order.length - 1}
                    aria-label={t('sidebar-move-down', { name: t(MAILBOX_LOOK[key].label) })}
                    onClick={() => move(key, 1)}
                  >
                    <Icon icon={ChevronDown} size={14} />
                  </button>
                </div>

                <label className="mailbox-show">
                  <input
                    type="checkbox"
                    checked={essential || !hidden}
                    // The one row nobody may hide. Disabled and checked rather
                    // than absent, so the list still reads as the sidebar does.
                    disabled={essential}
                    onChange={() => toggle(key)}
                  />
                  {/* The same glyph the rail draws. Reading this pane means
                      matching it against the sidebar beside it, and a name on
                      its own makes that a translation exercise. */}
                  <Icon icon={MAILBOX_LOOK[key].glyph} size={14} />
                  <span className={hidden && !essential ? 'mailbox-name off' : 'mailbox-name'}>
                    {t(MAILBOX_LOOK[key].label)}
                  </span>
                </label>

                <Pill
                  value={countFor(arrangement, key)}
                  onChange={(v) => setCount(key, v)}
                  label={t('sidebar-count-for', { name: t(MAILBOX_LOOK[key].label) })}
                  options={MODES.map((m) => ({ value: m.value, label: t(m.label) }))}
                />

              </li>
            );
          })}
        </ul>
      </section>

      {/* Folders have no rows here — there can be hundreds and they are yours
          to arrange in the sidebar itself. What they do have is a number, and
          it needs somewhere to be set. */}
      <section className="field">
        <div className="flabel">{t('sidebar-folders')}</div>
        <p className="fhelp">{t('sidebar-folders-help')}</p>
        <Pill
          value={countFor(arrangement, 'folders')}
          onChange={(v) => setCount('folders', v)}
          options={MODES.map((m) => ({ value: m.value, label: t(m.label) }))}
        />
      </section>
    </div>
  );
}
