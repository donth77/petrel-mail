import { useEffect, useRef, useState } from 'react';
import { ArrowLeft, Check, ExternalLink, Loader2 } from 'lucide-react';
import { api, type AccountSetup, type Discovered, type Server } from '../lib/api';
import { Icon } from './Icon';
import { t } from '../lib/strings';

/**
 * Adding an account: address, confirm, first sync.
 *
 * Three screens because the common case is three decisions — which address,
 * "yes that's my provider", and a password — and a form that asks for a
 * hostname first is asking the wrong person. The manual form exists, and is
 * one click away on every screen, but it is the escape hatch rather than the
 * door.
 *
 * The one thing said on every screen: nothing leaves this Mac except to the
 * provider itself. The password goes to the keychain after the connection
 * test passes, and not before.
 */

type Step =
  | { kind: 'ask' }
  | { kind: 'looking'; address: string }
  | { kind: 'confirm'; address: string; found: Discovered }
  | { kind: 'manual'; address: string; found: Discovered | null; imap: Server; smtp: Server }
  | { kind: 'syncing'; address: string };

/** The domain half of an address, for the confirm screen's sentence. */
function domainOf(address: string): string {
  return address.slice(address.lastIndexOf('@') + 1).toLowerCase();
}

export function Onboarding({ onDone }: { onDone: (added: { id: number; email: string } | null) => void }) {
  const [step, setStep] = useState<Step>({ kind: 'ask' });
  const [address, setAddress] = useState('');
  const [password, setPassword] = useState('');
  const [username, setUsername] = useState('');
  const [testing, setTesting] = useState<'idle' | 'imap' | 'smtp' | 'ok' | string>('idle');
  const [error, setError] = useState<string | null>(null);
  const addressRef = useRef<HTMLInputElement>(null);
  // What was stored, so the caller can show it when the person is done.
  const [added, setAdded] = useState<{ id: number; email: string } | null>(null);

  useEffect(() => {
    if (step.kind === 'ask') addressRef.current?.focus();
  }, [step.kind]);

  const lookUp = async () => {
    const a = address.trim();
    if (!a.includes('@')) return;
    setError(null);
    setUsername(a);
    setStep({ kind: 'looking', address: a });
    try {
      const found = await api.discoverAccount(a);
      if (found) setStep({ kind: 'confirm', address: a, found });
      else await goManual(a, null);
    } catch (e) {
      // "Proton Mail does not offer IMAP…" is an answer, not a failure: the
      // provider cannot be reached by any mail client, and the right thing
      // is to say so rather than open a form full of servers that do not
      // exist. Anything else is a real failure and reads as one.
      const msg = String(e).replace(/^Error:\s*/, '');
      setError(/does not offer IMAP/.test(msg) ? t('onb-no-imap', { detail: msg }) : msg);
      setStep({ kind: 'ask' });
    }
  };

  const goManual = async (a: string, found: Discovered | null) => {
    const guessed = found ? [found.imap, found.smtp] : await api.guessServers(a);
    const [imap, smtp] = guessed ?? [
      { host: '', port: 993, tls: true },
      { host: '', port: 465, tls: true },
    ];
    setTesting('idle');
    setStep({ kind: 'manual', address: a, found, imap, smtp });
  };

  const setupFrom = (a: string, imap: Server, smtp: Server, provider: string): AccountSetup => ({
    email: a,
    username: username.trim() || a,
    password,
    imap_host: imap.host.trim(),
    imap_port: imap.port,
    smtp_host: smtp.host.trim(),
    smtp_port: smtp.port,
    provider,
  });

  /** Test, then store, then sync. Storing never happens without the test. */
  const signIn = async (setup: AccountSetup) => {
    setError(null);
    // One server at a time, each named while it runs. Some providers take
    // several seconds per login, and a single spinner over both reads as
    // stuck by the time the second begins.
    try {
      setTesting('imap');
      await api.testAccount(setup, 'imap');
      setTesting('smtp');
      await api.testAccount(setup, 'smtp');
      setTesting('ok');
    } catch (e) {
      // The command's message already says which server and why; an
      // "Error:" prefix from the bridge on top of "Could not sign in —" reads
      // as a stutter.
      setTesting(String(e).replace(/^Error:\s*/, ''));
      return;
    }
    try {
      const id = await api.addAccount(setup);
      setAdded({ id, email: setup.email });
      setStep({ kind: 'syncing', address: setup.email });
    } catch (e) {
      setError(t('onb-add-failed', { error: String(e) }));
    }
  };

  return (
    <div className="onboarding" role="dialog" aria-label={t('onb-step-1')}>
      <div className="onb-card">
        {step.kind === 'ask' && (
          <form
            onSubmit={(e) => {
              e.preventDefault();
              void lookUp();
            }}
          >
            <div className="onb-step">{t('onb-step-1')}</div>
            <h1 className="onb-title">{t('onb-ask')}</h1>
            <p className="onb-help">{t('onb-ask-help')}</p>
            <input
              ref={addressRef}
              className="onb-field"
              type="email"
              autoComplete="email"
              autoCorrect="off"
              autoCapitalize="none"
              spellCheck={false}
              // Generic on purpose: a placeholder that reads as a particular
              // person's address invites typing it. `.example` is reserved and
              // can never be real.
              placeholder="you@example.com"
              value={address}
              onChange={(e) => setAddress(e.target.value)}
              onKeyDown={(e) => e.key !== 'Escape' && e.stopPropagation()}
            />
            <p className="onb-quiet">{t('onb-nothing-sent')}</p>
            {error && <p className="onb-error">{error}</p>}
            <div className="onb-acts">
              <button
                type="button"
                className="linkish"
                onClick={() => void goManual(address.trim() || 'you@example.com', null)}
              >
                {t('onb-manual')}
              </button>
              <span className="spacer" />
              <button type="submit" className="reply primary" disabled={!address.includes('@')}>
                {t('onb-continue')}
              </button>
            </div>
          </form>
        )}

        {step.kind === 'looking' && (
          <div>
            <div className="onb-step">{t('onb-step-1')}</div>
            <p className="onb-help">
              <Icon icon={Loader2} size={14} className="spin" /> {t('onb-looking')}
            </p>
          </div>
        )}

        {step.kind === 'confirm' && (
          <form
            onSubmit={(e) => {
              e.preventDefault();
              void signIn(setupFrom(step.address, step.found.imap, step.found.smtp, step.found.provider));
            }}
          >
            <div className="onb-step">{t('onb-step-2')}</div>
            <h1 className="onb-title">{t('onb-found', { provider: step.found.provider })}</h1>
            <p className="onb-help">
              {t(step.found.via === 'mx' ? 'onb-found-mx' : 'onb-found-domain', {
                domain: domainOf(step.address),
                provider: step.found.provider,
              })}
            </p>
            <p className="onb-body">{t('onb-direct', { provider: step.found.provider })}</p>
            <p className="onb-body">{t('onb-stored')}</p>

            {/* Said before the password box rather than after the attempt.
                Microsoft has been retiring password sign-in for mail, and
                Petrel sends LOGIN and nothing else — so for most of these
                accounts no password will work, whatever is typed here.
                Warned rather than blocked: some tenants still allow one, and
                deciding for somebody whose account might be the exception is
                not this screen's call to make. */}
            {step.found.auth === 'oauth-required' && (
              <p className="onb-warn">{t('onb-oauth-required', { provider: step.found.provider })}</p>
            )}

            <label className="onb-label" htmlFor="onb-pass">
              {t(step.found.auth === 'app-password' ? 'onb-app-password' : 'onb-password')}
            </label>
            <input
              id="onb-pass"
              className="onb-field"
              type="password"
              autoComplete="current-password"
              autoFocus
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              onKeyDown={(e) => e.key !== 'Escape' && e.stopPropagation()}
            />
            <p className="onb-quiet">
              {step.found.auth === 'app-password'
                ? t('onb-app-password-help', { provider: step.found.provider })
                : step.found.auth === 'oauth-required'
                  ? t('onb-oauth-help')
                  : t('onb-password-help')}
              {step.found.app_password_url && (
                <>
                  {' '}
                  <button
                    type="button"
                    className="linkish"
                    onClick={() => void api.openExternal(step.found.app_password_url!)}
                  >
                    {t('onb-app-password-link')} <Icon icon={ExternalLink} size={11} />
                  </button>
                </>
              )}
            </p>
            <TestLine state={testing} />
            {error && <p className="onb-error">{error}</p>}
            <div className="onb-acts">
              <button type="button" className="linkish" onClick={() => setStep({ kind: 'ask' })}>
                <Icon icon={ArrowLeft} size={13} /> {t('onb-back')}
              </button>
              <button
                type="button"
                className="linkish"
                onClick={() => void goManual(step.address, step.found)}
              >
                {t('onb-not-this', { provider: step.found.provider })} {t('onb-setup-manually')}
              </button>
              <span className="spacer" />
              <button
                type="submit"
                className="reply primary"
                disabled={!password || testing === 'imap' || testing === 'smtp'}
              >
                {t('onb-sign-in')}
              </button>
            </div>
          </form>
        )}

        {step.kind === 'manual' && (
          <form
            onSubmit={(e) => {
              e.preventDefault();
              void signIn(
                setupFrom(step.address, step.imap, step.smtp, step.found?.provider ?? step.imap.host),
              );
            }}
          >
            <div className="onb-step">{t('onb-servers')}</div>
            {!step.found && (
              <>
                <h1 className="onb-title">{t('onb-not-found', { domain: domainOf(step.address) })}</h1>
                <p className="onb-help">{t('onb-not-found-help')}</p>
              </>
            )}
            {step.found && <p className="onb-help">{t('onb-servers-help')}</p>}

            <label className="onb-label" htmlFor="onb-user">
              {t('onb-username')}
            </label>
            <input
              id="onb-user"
              className="onb-field"
              autoComplete="username"
              autoCorrect="off"
              autoCapitalize="none"
              spellCheck={false}
              value={username}
              onChange={(e) => setUsername(e.target.value)}
              onKeyDown={(e) => e.key !== 'Escape' && e.stopPropagation()}
            />
            <label className="onb-label" htmlFor="onb-pass-m">
              {t('onb-password')}
            </label>
            <input
              id="onb-pass-m"
              className="onb-field"
              type="password"
              autoComplete="current-password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              onKeyDown={(e) => e.key !== 'Escape' && e.stopPropagation()}
            />
            <p className="onb-quiet">{t('onb-password-help')}</p>

            <div className="onb-servers">
              <ServerFields
                label={t('onb-incoming')}
                value={step.imap}
                onChange={(imap) => setStep({ ...step, imap })}
              />
              <ServerFields
                label={t('onb-outgoing')}
                value={step.smtp}
                onChange={(smtp) => setStep({ ...step, smtp })}
              />
            </div>
            <TestLine state={testing} />
            {error && <p className="onb-error">{error}</p>}
            <div className="onb-acts">
              <button type="button" className="linkish" onClick={() => setStep({ kind: 'ask' })}>
                <Icon icon={ArrowLeft} size={13} /> {t('onb-back')}
              </button>
              <span className="spacer" />
              {testing !== 'idle' && testing !== 'imap' && testing !== 'smtp' && (
                <button
                  type="button"
                  className="reply"
                  onClick={() =>
                    void signIn(
                      setupFrom(step.address, step.imap, step.smtp, step.found?.provider ?? step.imap.host),
                    )
                  }
                >
                  {t('onb-retest')}
                </button>
              )}
              <button
                type="submit"
                className="reply primary"
                disabled={!password || !step.imap.host || !step.smtp.host || testing === 'imap' || testing === 'smtp'}
              >
                {t('onb-sign-in')}
              </button>
            </div>
          </form>
        )}

        {step.kind === 'syncing' && <FirstSync address={step.address} onDone={() => onDone(added)} />}
      </div>
    </div>
  );
}

/** "Reached both servers…" / a reason / nothing, below the sign-in button. */
function TestLine({ state }: { state: 'idle' | 'imap' | 'smtp' | 'ok' | string }) {
  if (state === 'idle') return null;
  if (state === 'imap' || state === 'smtp')
    return (
      <p className="onb-test" aria-live="polite">
        <Icon icon={Loader2} size={13} className="spin" />{' '}
        {t(state === 'imap' ? 'onb-testing-imap' : 'onb-testing-smtp')}
      </p>
    );
  if (state === 'ok')
    return (
      <p className="onb-test ok">
        <Icon icon={Check} size={13} /> {t('onb-tested')}
      </p>
    );
  return <p className="onb-test bad">{t('onb-failed', { error: state })}</p>;
}

function ServerFields({
  label,
  value,
  onChange,
}: {
  label: string;
  value: Server;
  onChange: (s: Server) => void;
}) {
  return (
    <div className="onb-server">
      <div className="onb-label">{label}</div>
      <div className="onb-server-row">
        <input
          className="onb-field"
          autoCorrect="off"
          autoCapitalize="none"
          spellCheck={false}
          value={value.host}
          onChange={(e) => onChange({ ...value, host: e.target.value })}
          onKeyDown={(e) => e.key !== 'Escape' && e.stopPropagation()}
          aria-label={t('onb-host-of', { what: label })}
        />
        <input
          className="onb-field onb-port"
          inputMode="numeric"
          value={value.port}
          onChange={(e) => onChange({ ...value, port: Number(e.target.value) || 0 })}
          onKeyDown={(e) => e.key !== 'Escape' && e.stopPropagation()}
          aria-label={t('onb-port-of', { what: label })}
        />
        <span className="onb-tls mono">TLS</span>
      </div>
    </div>
  );
}

/** Step 3: the sync, watched. */
function FirstSync({ address, onDone }: { address: string; onDone: () => void }) {
  const [count, setCount] = useState(0);
  const [total, setTotal] = useState(0);
  const [seeding, setSeeding] = useState(true);
  useEffect(() => {
    let live = true;
    const tick = () =>
      api
        .status()
        .then((s) => {
          if (!live) return;
          setCount(s.count);
          setTotal(s.server_total);
          setSeeding(s.seeding);
        })
        .catch(() => {});
    void tick();
    const h = setInterval(tick, 700);
    return () => {
      live = false;
      clearInterval(h);
    };
  }, []);
  return (
    <div>
      <div className="onb-step">{t('onb-step-3')}</div>
      <h1 className="onb-title">{t('onb-getting')}</h1>
      <p className="onb-help">{t('onb-getting-help')}</p>
      <p className="onb-count mono">
        {total > 0
          ? t('onb-progress', { count: count.toLocaleString(), total: total.toLocaleString() })
          : t('onb-progress-unknown', { count: count.toLocaleString() })}
        {count > 0 && <> · {t('onb-inbox-ready')}</>}
      </p>
      <p className="onb-quiet">{address}</p>
      <div className="onb-acts">
        <span className="spacer" />
        <button type="button" className="reply primary" onClick={onDone} disabled={seeding && count === 0}>
          {t('onb-start')}
        </button>
      </div>
    </div>
  );
}
