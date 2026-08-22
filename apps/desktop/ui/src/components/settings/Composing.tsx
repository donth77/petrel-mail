import { useSettings } from '../../lib/settings';
import { t } from '../../lib/strings';

/** Undo-send windows, in seconds. `0` sends at once. */
const WINDOWS: { value: string; note?: string }[] = [
  { value: '0', note: 'compose-undo-off' },
  { value: '5' },
  { value: '10', note: 'compose-undo-default' },
  { value: '20' },
  { value: '30', note: 'compose-undo-most' },
];

export function Composing() {
  const { settings, set } = useSettings();

  return (
    <div className="pane-body">
      <h1 className="pane-title">{t('settings-composing')}</h1>

      <section className="field">
        <div className="flabel">{t('compose-undo')}</div>
        <p className="fhelp">{t('compose-undo-help')}</p>
        <div className="seg seg-wide" role="group">
          {WINDOWS.map((w) => (
            <button
              key={w.value}
              type="button"
              className={settings.undoSendSeconds === w.value ? 'on' : undefined}
              aria-pressed={settings.undoSendSeconds === w.value}
              onClick={() => set('undoSendSeconds', w.value)}
            >
              <span className="mono seg-value">
                {w.value === '0' ? t('compose-undo-off-label') : `${w.value}s`}
              </span>
              {/* Marking the default is worth the space: a list of five numbers
                  gives no clue which one the app was designed around. */}
              <span className="tiny">{w.note ? t(w.note as 'compose-undo-default') : ''}</span>
            </button>
          ))}
        </div>
      </section>

      <section className="field">
        <div className="flabel">{t('compose-writing')}</div>
        <label className="check">
          <input
            type="checkbox"
            checked={settings.warnMissingAttachment === 'on'}
            onChange={(e) => set('warnMissingAttachment', e.target.checked ? 'on' : 'off')}
          />
          <span>{t('compose-warn-attachment')}</span>
        </label>
        <p className="fhelp">{t('compose-warn-attachment-help')}</p>
      </section>

      <section className="field">
        <div className="flabel">{t('compose-replying')}</div>
        <p className="fhelp">{t('compose-reply-default-help')}</p>
        <div className="seg" role="group">
          {(
            [
              ['reply', t('reader-reply')],
              ['reply-all', t('reader-reply-all')],
            ] as const
          ).map(([value, label]) => (
            <button
              key={value}
              type="button"
              className={settings.replyDefault === value ? 'on' : undefined}
              aria-pressed={settings.replyDefault === value}
              onClick={() => set('replyDefault', value)}
            >
              {label}
            </button>
          ))}
        </div>
      </section>
    </div>
  );
}
