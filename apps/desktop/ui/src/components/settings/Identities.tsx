import { useEffect, useState } from 'react';
import { api, type Identity } from '../../lib/api';
import { AccountNote } from './AccountNote';
import { t } from '../../lib/strings';

/**
 * One identity per account, until aliases can be checked with the provider.
 *
 * Offering to send as an address the server will reject is worse than not
 * offering it — the failure arrives after the message looks sent, which is the
 * worst moment to find out.
 */
export function Identities({ onMessage }: { onMessage: (text: string) => void }) {
  const [identity, setIdentity] = useState<Identity | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    api
      .identity()
      .then((i) => live && setIdentity(i))
      .catch((e) => live && setError(String(e)));
    return () => {
      live = false;
    };
  }, []);

  // Saved on change rather than behind a Save button: every other setting in
  // this window applies as you touch it, and one pane that does not is a trap.
  const save = (next: Identity) => {
    setIdentity(next);
    api
      .setIdentity(next.display_name, next.signature, next.signature_on_reply)
      .catch((e) => onMessage(t('identity-save-failed', { error: String(e) })));
  };

  if (error) {
    return (
      <div className="pane-body">
        <h1 className="pane-title">{t('settings-identities')}</h1>
        <AccountNote />
        <p className="fhelp">{error}</p>
      </div>
    );
  }
  if (!identity) return <div className="pane-body" />;

  return (
    <div className="pane-body">
      <h1 className="pane-title">{t('settings-identities')}</h1>
      <AccountNote />

      <section className="field">
        <div className="flabel">{t('identity-sending-as')}</div>
        <p className="fhelp">{t('identity-alias-note')}</p>
        <label className="stack">
          <span className="tiny-label">{t('identity-name')}</span>
          <input
            className="text-input"
            value={identity.display_name}
            placeholder={t('identity-name-placeholder')}
            onChange={(e) => save({ ...identity, display_name: e.target.value })}
          />
        </label>
        {/* The address is what the account signs in as, so it is shown rather
            than offered for editing — changing it here would not change who
            the server thinks you are. */}
        <p className="identity-preview mono">
          {identity.display_name
            ? `${identity.display_name} <${identity.address}>`
            : identity.address}
        </p>
      </section>

      <section className="field">
        <div className="flabel">{t('identity-signature')}</div>
        <textarea
          className="text-input signature-input"
          value={identity.signature}
          placeholder={t('identity-signature-placeholder')}
          onChange={(e) => save({ ...identity, signature: e.target.value })}
        />
        <label className="check">
          <input
            type="checkbox"
            checked={identity.signature_on_reply}
            onChange={(e) => save({ ...identity, signature_on_reply: e.target.checked })}
          />
          <span>{t('identity-signature-replies')}</span>
        </label>
        <p className="fhelp">{t('identity-signature-replies-help')}</p>
      </section>
    </div>
  );
}
