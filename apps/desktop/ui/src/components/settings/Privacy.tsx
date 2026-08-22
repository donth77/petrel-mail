import { useEffect, useState } from 'react';
import { ShieldCheck, X } from 'lucide-react';
import { api } from '../../lib/api';
import { useSettings } from '../../lib/settings';
import { Icon } from '../Icon';
import { t } from '../../lib/strings';

export function Privacy() {
  const { settings, set } = useSettings();
  const blocked = settings.blockRemoteContent === 'on';
  const [trusted, setTrusted] = useState<string[]>([]);

  // Re-read whenever blocking is switched back on: the list is meaningless
  // while everything is allowed, and stale by the time it matters again.
  useEffect(() => {
    let live = true;
    api
      .trustedSenders()
      .then((rows) => live && setTrusted(rows))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [blocked]);

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

      {/* Only while blocking is on. A list of exceptions to a rule that is not
          in force reads as though it were doing something. */}
      {blocked && (
        <section className="field">
          <div className="flabel">{t('privacy-trusted')}</div>
          <p className="fhelp">{t('privacy-trusted-help')}</p>
          {trusted.length === 0 ? (
            <p className="fhelp">{t('privacy-trusted-none')}</p>
          ) : (
            <ul className="trusted-list">
              {trusted.map((addr) => (
                <li key={addr}>
                  <span className="mono clip">{addr}</span>
                  <button
                    type="button"
                    className="linkish"
                    aria-label={t('privacy-untrust', { addr })}
                    onClick={() =>
                      void api
                        .untrustSender(addr)
                        .then(() => setTrusted((prev) => prev.filter((a) => a !== addr)))
                        .catch(() => {})
                    }
                  >
                    <Icon icon={X} size={13} />
                  </button>
                </li>
              ))}
            </ul>
          )}
        </section>
      )}

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
