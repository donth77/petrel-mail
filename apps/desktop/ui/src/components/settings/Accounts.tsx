import { useEffect, useState } from 'react';
import { api, type Account } from '../../lib/api';
import { count as fmtCount, listTime } from '../../lib/format';
import { t } from '../../lib/strings';

const COLORS = ['#0E7C86', '#9A6B1F', '#6B7F87', '#3B6EA5', '#6B5CA5', '#5E7C4A'];
const ROLES = ['archive', 'sent', 'drafts', 'spam', 'trash'] as const;

export function Accounts() {
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [selected, setSelected] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = () =>
    api
      .accounts()
      .then((a) => {
        setAccounts(a);
        setError(null);
        setSelected((cur) => (a.some((x) => x.id === cur) ? cur : (a[0]?.id ?? null)));
      })
      .catch((err: unknown) => setError(String(err)));

  useEffect(() => {
    void load();
  }, []);

  const account = accounts.find((a) => a.id === selected) ?? null;

  if (error) {
    return (
      <div className="pane-body">
        <h1 className="pane-title">{t('settings-accounts')}</h1>
        <div className="empty">
          <h2 style={{ color: 'var(--danger)' }}>{t('accounts-failed')}</h2>
          <p className="mono" style={{ fontSize: 11.5 }}>{error}</p>
        </div>
      </div>
    );
  }

  return (
    <div className="pane-body">
      <h1 className="pane-title">{t('settings-accounts')}</h1>

      <section className="field">
        <div className="field-head">
          <div className="flabel">{t('accounts-yours')}</div>
          {/* No button until it can do something. Adding an account needs
              credentials in the keychain and a sync task of its own, which arrive
              with the provider work; a control that only apologises is worse than
              an honest sentence. */}
          <p className="fhelp">{t('accounts-add-later')}</p>
        </div>
        <div className="account-list">
          {accounts.map((a) => (
            <button
              key={a.id}
              type="button"
              className="account-row"
              aria-current={a.id === selected ? 'true' : undefined}
              onClick={() => setSelected(a.id)}
            >
              <span className="dot" style={{ background: a.color || 'var(--ink3)' }} />
              <span className="account-main">
                <span className="account-email clip">{a.email}</span>
                <span className="tiny">
                  {a.display_name || a.kind}
                  {a.newest_ms ? ` · ${t('accounts-synced', { when: listTime(a.newest_ms) })}` : ''}
                </span>
              </span>
              <span className="mono tiny">
                {a.unread_count > 0 ? fmtCount(a.unread_count) : '—'}
              </span>
            </button>
          ))}
          {accounts.length === 0 && <p className="fhelp">{t('accounts-none')}</p>}
        </div>
      </section>

      {account && (
        <>
          <section className="field">
            <div className="flabel">{account.email}</div>
            <p className="fhelp">
              {t('accounts-storage', { count: fmtCount(account.message_count) })}
            </p>
            <div className="box">
              <div className="row2">
                <div className="t">
                  <b>{t('accounts-colour')}</b>
                  <span>{t('accounts-colour-help')}</span>
                </div>
                <div className="dotrow">
                  {COLORS.map((c) => (
                    <button
                      key={c}
                      type="button"
                      className={`acc sm${account.color === c ? ' on' : ''}`}
                      style={{ background: c }}
                      aria-label={c}
                      aria-pressed={account.color === c}
                      onClick={() => {
                        api
                          .setAccountColor(account.id, c)
                          .then(() => {
                            void api.log(`set_account_color ok account=${account.id} ${c}`);
                            return load();
                          })
                          .catch((err: unknown) => {
                            // Never silent: a write that fails and a write that
                            // changes nothing visible look identical otherwise.
                            setError(String(err));
                            void api.log(`set_account_color FAILED: ${err}`);
                          });
                      }}
                    />
                  ))}
                </div>
              </div>

              <div className="row2">
                <div className="t">
                  <b>{t('accounts-keep')}</b>
                  {/* Q24 in one line: what happens here when the server forgets. */}
                  <span>
                    {account.local_archive ? t('accounts-keep-archive') : t('accounts-keep-mirror')}
                  </span>
                </div>
                <div className="pill">
                  <button
                    type="button"
                    className={!account.local_archive ? 'on' : undefined}
                    onClick={() => {
                      api
                        .setAccountArchive(account.id, false)
                        .then(load)
                        .catch((err: unknown) => setError(String(err)));
                    }}
                  >
                    {t('accounts-mirror')}
                  </button>
                  <button
                    type="button"
                    className={account.local_archive ? 'on' : undefined}
                    onClick={() => {
                      api
                        .setAccountArchive(account.id, true)
                        .then(load)
                        .catch((err: unknown) => setError(String(err)));
                    }}
                  >
                    {t('accounts-archive')}
                  </button>
                </div>
              </div>
            </div>
          </section>

          <section className="field last">
            <div className="flabel">{t('accounts-folders')}</div>
            <p className="fhelp">{t('accounts-folders-help')}</p>
            {account.folders.length > 0 ? (
              <div className="folder-grid">
                {ROLES.map((role) => {
                  const f = account.folders.find((x) => x.role === role);
                  return (
                    <div className="folder-cell" key={role}>
                      <div className="tiny">{t(`folder-${role}` as never)}</div>
                      <div className="clip folder-path">{f?.path ?? t('folder-unmapped')}</div>
                    </div>
                  );
                })}
              </div>
            ) : (
              <p className="fhelp folder-none">{t('accounts-folders-none')}</p>
            )}
          </section>
        </>
      )}
    </div>
  );
}
