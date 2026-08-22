import { useSettings } from '../../lib/settings';
import { postDesktopNotification } from '../../lib/notify';
import { t } from '../../lib/strings';

/** Pause options, as offsets from now. `0` clears the pause. */
const PAUSES: { label: string; ms: number }[] = [
  { label: t('notify-pause-off'), ms: 0 },
  { label: t('notify-pause-hour'), ms: 60 * 60 * 1000 },
  { label: t('notify-pause-tomorrow'), ms: 0 },
];

export function Notifications() {
  const { settings, set } = useSettings();

  const pausedUntil = Number(settings.notifyPausedUntil) || 0;
  const paused = pausedUntil > Date.now();

  /** Tomorrow morning, not "24 hours from now": a pause you set at 11pm should
   *  end when your day starts, not at 11pm the following night. */
  const tomorrowMorning = () => {
    const d = new Date();
    d.setDate(d.getDate() + 1);
    d.setHours(8, 0, 0, 0);
    return d.getTime();
  };

  return (
    <div className="pane-body">
      <h1 className="pane-title">{t('settings-notifications')}</h1>

      <section className="field">
        <div className="flabel">{t('notify-pause')}</div>
        <p className="fhelp">
          {paused
            ? t('notify-paused-until', {
                when: new Date(pausedUntil).toLocaleString(undefined, {
                  weekday: 'short',
                  hour: 'numeric',
                  minute: '2-digit',
                }),
              })
            : t('notify-pause-help')}
        </p>
        <div className="seg" role="group">
          <button
            type="button"
            className={!paused ? 'on' : undefined}
            aria-pressed={!paused}
            onClick={() => set('notifyPausedUntil', '0')}
          >
            {PAUSES[0].label}
          </button>
          <button
            type="button"
            className={paused && pausedUntil < tomorrowMorning() ? 'on' : undefined}
            onClick={() => set('notifyPausedUntil', String(Date.now() + PAUSES[1].ms))}
          >
            {PAUSES[1].label}
          </button>
          <button
            type="button"
            className={paused && pausedUntil >= tomorrowMorning() ? 'on' : undefined}
            onClick={() => set('notifyPausedUntil', String(tomorrowMorning()))}
          >
            {PAUSES[2].label}
          </button>
        </div>
      </section>

      <section className="field">
        <div className="flabel">{t('notify-level')}</div>
        <p className="fhelp">{t('notify-level-help')}</p>
        <div className="seg" role="group">
          {(
            [
              ['all', t('notify-level-all')],
              ['priority', t('notify-level-priority')],
              ['none', t('notify-level-none')],
            ] as const
          ).map(([value, label]) => (
            <button
              key={value}
              type="button"
              className={settings.notifyLevel === value ? 'on' : undefined}
              aria-pressed={settings.notifyLevel === value}
              onClick={() => set('notifyLevel', value)}
            >
              {label}
            </button>
          ))}
        </div>
      </section>

      <section className="field">
        <div className="flabel">{t('badges')}</div>
        <p className="fhelp">{t('badges-help')}</p>
        <div className="seg" role="group">
          {(
            [
              ['unread', t('badges-unread')],
              ['total', t('badges-total')],
              ['off', t('badges-off')],
            ] as const
          ).map(([value, label]) => (
            <button
              key={value}
              type="button"
              className={settings.badges === value ? 'on' : undefined}
              aria-pressed={settings.badges === value}
              onClick={() => set('badges', value)}
            >
              {label}
            </button>
          ))}
        </div>
      </section>

      <section className="field">
        <div className="flabel">{t('notify-desktop')}</div>
        <p className="fhelp">{t('notify-desktop-help')}</p>
        <div className="seg" role="group">
          <button
            type="button"
            className={settings.notifyDesktop === 'on' ? 'on' : undefined}
            aria-pressed={settings.notifyDesktop === 'on'}
            onClick={() => set('notifyDesktop', 'on')}
          >
            {t('notify-desktop-on')}
          </button>
          <button
            type="button"
            className={settings.notifyDesktop === 'off' ? 'on' : undefined}
            aria-pressed={settings.notifyDesktop === 'off'}
            onClick={() => set('notifyDesktop', 'off')}
          >
            {t('notify-desktop-off')}
          </button>
        </div>
        {/* The OS has the final say and can refuse silently, so the only honest
            way to tell someone notifications work is to send one. */}
        <button
          type="button"
          className="fbtn"
          onClick={() => void postDesktopNotification(t('app-name'), t('notify-test-body'))}
        >
          {t('notify-test')}
        </button>
      </section>
    </div>
  );
}
