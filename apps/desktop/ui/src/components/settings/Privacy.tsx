import { ShieldCheck } from 'lucide-react';
import { useSettings } from '../../lib/settings';
import { Icon } from '../Icon';
import { t } from '../../lib/strings';

export function Privacy() {
  const { settings, set } = useSettings();
  const blocked = settings.blockRemoteContent === 'on';

  return (
    <div className="pane-body">
      <h1 className="pane-title">{t('settings-privacy')}</h1>

      <section className="field">
        <div className="flabel">{t('privacy-remote')}</div>
        <p className="fhelp">{t('privacy-remote-help')}</p>
        <div className="seg" role="group">
          <button
            type="button"
            className={blocked ? 'on' : undefined}
            aria-pressed={blocked}
            onClick={() => set('blockRemoteContent', 'on')}
          >
            {t('privacy-remote-block')}
          </button>
          <button
            type="button"
            className={!blocked ? 'on' : undefined}
            aria-pressed={!blocked}
            onClick={() => set('blockRemoteContent', 'off')}
          >
            {t('privacy-remote-allow')}
          </button>
        </div>
        {!blocked && (
          // Said plainly rather than left to be inferred: loading a remote
          // image tells the sender the message was opened, by whom, and when.
          <p className="fhelp warn">{t('privacy-remote-warning')}</p>
        )}
      </section>

      {/* Facts, not controls. These are properties of how messages are
          rendered, and there is no version of Petrel where they are off — a
          switch implying otherwise would be a lie about the sandbox. */}
      <section className="field">
        <div className="flabel">{t('privacy-always')}</div>
        <ul className="privacy-facts">
          <li>
            <Icon icon={ShieldCheck} size={14} />
            <span>{t('privacy-fact-scripts')}</span>
          </li>
          <li>
            <Icon icon={ShieldCheck} size={14} />
            <span>{t('privacy-fact-links')}</span>
          </li>
          <li>
            <Icon icon={ShieldCheck} size={14} />
            <span>{t('privacy-fact-forms')}</span>
          </li>
          <li>
            <Icon icon={ShieldCheck} size={14} />
            <span>{t('privacy-fact-referrer')}</span>
          </li>
        </ul>
      </section>
    </div>
  );
}
