import { useEffect, useState } from 'react';
import { Download, Upload } from 'lucide-react';
import { api, type Account, type StorageReport } from '../../lib/api';
import { fileSize } from '../../lib/format';
import { Icon } from '../Icon';
import { t } from '../../lib/strings';

/** Views worth exporting, in the order someone would think of them. */
const SCOPES: { view: string; label: 'mailbox-inbox' | 'mailbox-archive' | 'mailbox-starred' }[] = [
  { view: 'inbox', label: 'mailbox-inbox' },
  { view: 'archive', label: 'mailbox-archive' },
  { view: 'starred', label: 'mailbox-starred' },
];

export function Storage({ onMessage }: { onMessage: (text: string) => void }) {
  const [report, setReport] = useState<StorageReport | null>(null);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let live = true;
    // Two requests, not one: the account list is what the export buttons
    // need and it is back at once, while the figures take a moment on a large
    // mailbox. Waiting for the figures would make the buttons wait too, and
    // they have no reason to.
    api
      .accounts()
      .then((a) => live && setAccounts(a))
      .catch((e) => live && setError(String(e)));
    api
      .storage()
      .then((r) => live && setReport(r))
      .catch((e) => live && setError(String(e)));
    return () => {
      live = false;
    };
  }, []);

  /**
   * Exports one account's view to a file the user picks.
   *
   * The save panel comes from the OS rather than Petrel choosing a location:
   * an export is a thing you take somewhere else, and the promise it exists to
   * keep would be a poor one if honouring it meant knowing where Petrel hides
   * its files. The account is in the suggested name for the same reason — the
   * file outlives the app that wrote it, and should say whose mail it is.
   */
  const exportTo = async (account: Account, view: string, label: string) => {
    setBusy(true);
    try {
      const { save } = await import('@tauri-apps/plugin-dialog');
      const path = await save({
        defaultPath: `petrel-${view}-${account.email}.mbox`,
        filters: [{ name: 'mbox', extensions: ['mbox'] }],
      });
      // Cancelling is an answer, not a failure.
      if (!path) return;
      const result = await api.exportMbox(account.id, view, path);
      const [written, skipped] = result.split('/');
      const vars = { count: written, view: label, account: account.email };
      onMessage(
        Number(skipped) > 0
          ? t('storage-exported-partial', { ...vars, skipped })
          : t('storage-exported', vars),
      );
    } catch (e) {
      onMessage(t('storage-export-failed', { error: String(e) }));
    } finally {
      setBusy(false);
    }
  };

  /** Imports mbox or .eml files into a local "Imported" folder. */
  const importFrom = async () => {
    setBusy(true);
    try {
      const { open } = await import('@tauri-apps/plugin-dialog');
      const picked = await open({
        multiple: true,
        filters: [{ name: 'Mail archives', extensions: ['mbox', 'mbx', 'eml'] }],
      });
      if (!picked) return;
      const paths = Array.isArray(picked) ? picked : [picked];
      const r = await api.importMail(paths);
      onMessage(
        r.failed > 0 || r.duplicates > 0
          ? t('storage-imported-mixed', {
              count: String(r.imported),
              duplicates: String(r.duplicates),
              failed: String(r.failed),
            })
          : t('storage-imported', { count: String(r.imported) }),
      );
    } catch (e) {
      onMessage(t('storage-import-failed', { error: String(e) }));
    } finally {
      setBusy(false);
    }
  };

  // The rows are laid out before the numbers arrive. The pane is what the
  // person selected; the figures are a detail of it, and a settings pane that
  // withholds its whole body until a background count finishes reads as the
  // click not having registered.
  const rows: { label: Parameters<typeof t>[0]; value: string | null }[] = [
    { label: 'storage-messages', value: report && report.messages.toLocaleString() },
    { label: 'storage-attachments', value: report && report.attachments.toLocaleString() },
    { label: 'storage-mail', value: report && fileSize(report.blob_bytes) },
    { label: 'storage-database', value: report && fileSize(report.database_bytes) },
    // Listed apart from the rest because it is the one figure that can be
    // thrown away and rebuilt from the mail.
    { label: 'storage-index', value: report && fileSize(report.index_bytes) },
  ];

  // The totals above are for the whole Mac. With one account that is the
  // account; with more, the split is the thing someone came here to see.
  const byAccount =
    report && accounts.length > 1
      ? report.accounts
          .map((s) => ({ s, a: accounts.find((a) => a.id === s.account_id) }))
          .filter((x): x is { s: (typeof report.accounts)[number]; a: Account } => !!x.a)
      : [];

  const accountLabel = (a: Account) => (
    <span className="storage-account">
      <span className="dot" style={{ background: a.color || 'var(--ink3)' }} />
      <span className="clip">{a.email}</span>
    </span>
  );

  const exportButtons = (account: Account | undefined) => (
    <div className="storage-actions">
      {SCOPES.map((s) => (
        <button
          key={s.view}
          type="button"
          className="fbtn"
          disabled={busy || !account}
          onClick={() => account && void exportTo(account, s.view, t(s.label))}
        >
          <Icon icon={Download} size={13} />
          {t(s.label)}
        </button>
      ))}
    </div>
  );

  return (
    <div className="pane-body">
      <h1 className="pane-title">{t('settings-storage')}</h1>

      <section className="field" aria-busy={!report && !error}>
        <div className="flabel">{t('storage-on-this-mac')}</div>
        {error ? (
          <p className="fhelp">{error}</p>
        ) : (
          <table className="storage-table">
            <tbody>
              {rows.map((r) => (
                <tr key={r.label}>
                  <td>{t(r.label)}</td>
                  <td className="mono">
                    {r.value ?? <span className="skel" aria-hidden="true" />}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </section>

      {byAccount.length > 0 && (
        <section className="field">
          <div className="flabel">{t('storage-by-account')}</div>
          <p className="fhelp">{t('storage-by-account-help')}</p>
          <table className="storage-table">
            <tbody>
              {byAccount.map(({ s, a }) => (
                <tr key={a.id}>
                  <td>{accountLabel(a)}</td>
                  <td className="mono num">
                    {t('storage-account-messages', { count: s.messages.toLocaleString() })}
                  </td>
                  <td className="mono num">{fileSize(s.blob_bytes)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </section>
      )}

      <section className="field">
        <div className="flabel">{t('storage-export')}</div>
        <p className="fhelp">{t('storage-export-help')}</p>
        {accounts.length > 1 ? (
          // A row per account: which mailbox a file holds is not something to
          // leave to whichever account happened to be on screen.
          accounts.map((a) => (
            <div key={a.id} className="storage-export-row">
              {accountLabel(a)}
              {exportButtons(a)}
            </div>
          ))
        ) : (
          exportButtons(accounts[0])
        )}
      </section>

      <section className="field">
        <div className="flabel">{t('storage-import')}</div>
        <p className="fhelp">{t('storage-import-help')}</p>
        <div className="storage-actions">
          <button type="button" className="fbtn" disabled={busy} onClick={() => void importFrom()}>
            <Icon icon={Upload} size={13} />
            {t('storage-import-button')}
          </button>
        </div>
      </section>
    </div>
  );
}
