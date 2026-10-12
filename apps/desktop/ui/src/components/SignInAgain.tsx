import { useEffect, useRef, useState } from 'react';
import { Dialog } from '@ariakit/react';
import { ChevronDown, ChevronRight, Loader2 } from 'lucide-react';
import { api, type AccountForm } from '../lib/api';
import { ServerFields } from './Onboarding';
import { Icon } from './Icon';
import { t } from '../lib/strings';
import { adviceText } from '../lib/advice';

/**
 * Signing an account in again: its password, and, behind a disclosure, the
 * address, username and servers it signs in with.
 *
 * The way back for an account whose password changed — Google and Apple
 * revoke app passwords when the account password changes — or one whose
 * password Petrel cannot read, as an account an import brought has. There
 * used to be none: the only way out was Remove account, which deletes mail
 * that exists nowhere else. Thunderbird and Apple Mail ask for the password
 * and keep everything; so does this.
 *
 * The provider is asked before anything is stored, as onboarding asks: a
 * password it refuses changes nothing, and the error says which server and
 * why. Closing the dialog while that runs does not undo a password the
 * provider accepts; the account then simply signs in, and says so.
 */
export function SignInAgain({
  account,
  onClose,
  onSignedIn,
}: {
  /** The account to sign in, or null when the dialog is closed. */
  account: { id: number; email: string } | null;
  onClose: () => void;
  onSignedIn: (email: string) => void;
}) {
  const [form, setForm] = useState<AccountForm | null>(null);
  const [password, setPassword] = useState('');
  const [servers, setServers] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const passRef = useRef<HTMLInputElement>(null);
  const open = account !== null;
  const id = account?.id;

  // Each opening starts from the account as stored. The password is never
  // read back: there is nothing to show, and nothing to show it to.
  useEffect(() => {
    if (id === undefined) return;
    let live = true;
    setForm(null);
    setPassword('');
    setServers(false);
    setBusy(false);
    setError(null);
    api
      .accountForm(id)
      .then((f) => live && setForm(f))
      .catch((e: unknown) => live && setError(String(e).replace(/^Error:\s*/, '')));
    return () => {
      live = false;
    };
  }, [id]);

  useEffect(() => {
    if (open && form) passRef.current?.focus();
  }, [open, form]);

  const signIn = async () => {
    if (!account || !form || !password || busy) return;
    setBusy(true);
    setError(null);
    const email = form.email.trim();
    try {
      await api.updateAccount(account.id, {
        email,
        username: form.username.trim() || email,
        password,
        imap_host: form.imap_host.trim(),
        imap_port: form.imap_port,
        smtp_host: form.smtp_host.trim(),
        smtp_port: form.smtp_port,
        provider: form.provider,
      });
      onSignedIn(email || account.email);
    } catch (e) {
      setError(adviceText(String(e).replace(/^Error:\s*/, '')));
    } finally {
      setBusy(false);
    }
  };

  // Fields keep their keys to themselves: a letter typed into a password box
  // must never reach the mailbox behind as a command.
  const keep = (e: React.KeyboardEvent) => {
    if (e.key !== 'Escape') e.stopPropagation();
  };

  return (
    <Dialog open={open} onClose={onClose} className="onboarding-dialog" backdrop={false} aria-label={account ? t('signin-title', { email: account.email }) : undefined}>
      <div
        className="onboarding-dim"
        onClick={(e) => {
          if (e.target === e.currentTarget) onClose();
        }}
      >
        {account && (
          <form
            className="onb-card signin-card"
            onSubmit={(e) => {
              e.preventDefault();
              void signIn();
            }}
          >
            <h1 className="onb-title">{t('signin-title', { email: account.email })}</h1>
            <p className="onb-help">{t('signin-help')}</p>
            <label className="onb-label" htmlFor="signin-pass">
              {t('onb-password')}
            </label>
            <input
              id="signin-pass"
              ref={passRef}
              className="onb-field"
              type="password"
              autoComplete="current-password"
              value={password}
              disabled={!form}
              onChange={(e) => setPassword(e.target.value)}
              onKeyDown={keep}
            />
            <p className="onb-quiet">{t('onb-password-help')}</p>

            <button
              type="button"
              className="linkish signin-disclose"
              aria-expanded={servers}
              aria-controls="signin-servers"
              disabled={!form}
              onClick={() => setServers((v) => !v)}
            >
              <Icon icon={servers ? ChevronDown : ChevronRight} size={13} /> {t('onb-servers')}
            </button>
            {servers && form && (
              <div id="signin-servers" className="signin-servers">
                <label className="onb-label" htmlFor="signin-address">
                  {t('signin-address')}
                </label>
                <input
                  id="signin-address"
                  className="onb-field"
                  type="email"
                  autoComplete="email"
                  autoCorrect="off"
                  autoCapitalize="none"
                  spellCheck={false}
                  value={form.email}
                  onChange={(e) => setForm({ ...form, email: e.target.value })}
                  onKeyDown={keep}
                />
                <label className="onb-label" htmlFor="signin-user">
                  {t('onb-username')}
                </label>
                <input
                  id="signin-user"
                  className="onb-field"
                  autoComplete="username"
                  autoCorrect="off"
                  autoCapitalize="none"
                  spellCheck={false}
                  value={form.username}
                  onChange={(e) => setForm({ ...form, username: e.target.value })}
                  onKeyDown={keep}
                />
                <div className="onb-servers">
                  <ServerFields
                    label={t('onb-incoming')}
                    value={{ host: form.imap_host, port: form.imap_port, tls: true }}
                    onChange={(s) => setForm({ ...form, imap_host: s.host, imap_port: s.port })}
                  />
                  <ServerFields
                    label={t('onb-outgoing')}
                    value={{ host: form.smtp_host, port: form.smtp_port, tls: true }}
                    onChange={(s) => setForm({ ...form, smtp_host: s.host, smtp_port: s.port })}
                  />
                </div>
              </div>
            )}

            {/* One region for both, mounted with the form and hidden while it
                has nothing to say: a polite region that appears along with
                its text is not reliably announced, WebKit least of all. */}
            <p
              className={error && !busy ? 'onb-test bad' : 'onb-test'}
              role="status"
              aria-live="polite"
              hidden={!busy && !error}
            >
              {busy ? (
                <>
                  <Icon icon={Loader2} size={13} className="spin" /> {t('signin-checking')}
                </>
              ) : error ? (
                t('onb-failed', { error })
              ) : null}
            </p>
            <div className="onb-acts">
              <span className="spacer" />
              <button type="button" className="reply" onClick={onClose}>
                {t('cancel')}
              </button>
              <button
                type="submit"
                className="reply primary"
                disabled={!form || !password || busy || !form.imap_host.trim() || !form.smtp_host.trim()}
              >
                {t('onb-sign-in')}
              </button>
            </div>
          </form>
        )}
      </div>
    </Dialog>
  );
}
