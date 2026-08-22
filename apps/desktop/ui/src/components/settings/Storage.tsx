import { useEffect, useState } from 'react';
import { Download } from 'lucide-react';
import { api, type StorageReport } from '../../lib/api';
import { fileSize } from '../../lib/format';
import { Icon } from '../Icon';
import { t } from '../../lib/strings';

/** Views worth exporting, in the order someone would think of them. */
const SCOPES: { view: string; label: string }[] = [
  { view: 'inbox', label: 'mailbox-inbox' },
  { view: 'archive', label: 'mailbox-archive' },
  { view: 'starred', label: 'mailbox-starred' },
];

export function Storage({ onMessage }: { onMessage: (text: string) => void }) {
  const [report, setReport] = useState<StorageReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let live = true;
    api
      .storage()
      .then((r) => live && setReport(r))
      .catch((e) => live && setError(String(e)));
    return () => {
      live = false;
    };
  }, []);

  /**
   * Exports one view to a file the user picks.
   *
   * The save panel comes from the OS rather than Petrel choosing a location:
   * an export is a thing you take somewhere else, and the promise it exists to
   * keep would be a poor one if honouring it meant knowing where Petrel hides
   * its files.
   */
  const exportTo = async (view: string, label: string) => {
    setBusy(true);
    try {
      const { save } = await import('@tauri-apps/plugin-dialog');
      const path = await save({
        defaultPath: `petrel-${view}.mbox`,
        filters: [{ name: 'mbox', extensions: ['mbox'] }],
      });
      // Cancelling is an answer, not a failure.
      if (!path) return;
      const result = await api.exportMbox(view, path);
      const [written, skipped] = result.split('/');
      onMessage(
        Number(skipped) > 0
          ? t('storage-exported-partial', { count: written, skipped, view: label })
          : t('storage-exported', { count: written, view: label }),
      );
    } catch (e) {
      onMessage(t('storage-export-failed', { error: String(e) }));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="pane-body">
      <h1 className="pane-title">{t('settings-storage')}</h1>

      <section className="field">
        <div className="flabel">{t('storage-on-this-mac')}</div>
        {error && <p className="fhelp">{error}</p>}
        {report && (
          <table className="storage-table">
            <tbody>
              <tr>
                <td>{t('storage-messages')}</td>
                <td className="mono">{report.messages.toLocaleString()}</td>
              </tr>
              <tr>
                <td>{t('storage-attachments')}</td>
                <td className="mono">{report.attachments.toLocaleString()}</td>
              </tr>
              <tr>
                <td>{t('storage-mail')}</td>
                <td className="mono">{fileSize(report.blob_bytes)}</td>
              </tr>
              <tr>
                <td>{t('storage-database')}</td>
                <td className="mono">{fileSize(report.database_bytes)}</td>
              </tr>
              <tr>
                {/* Listed apart from the rest because it is the one figure that
                    can be thrown away and rebuilt from the mail. */}
                <td>{t('storage-index')}</td>
                <td className="mono">{fileSize(report.index_bytes)}</td>
              </tr>
            </tbody>
          </table>
        )}
      </section>

      <section className="field">
        <div className="flabel">{t('storage-export')}</div>
        <p className="fhelp">{t('storage-export-help')}</p>
        <div className="storage-actions">
          {SCOPES.map((s) => (
            <button
              key={s.view}
              type="button"
              className="fbtn"
              disabled={busy}
              onClick={() => void exportTo(s.view, t(s.label as 'mailbox-inbox'))}
            >
              <Icon icon={Download} size={13} />
              {t(s.label as 'mailbox-inbox')}
            </button>
          ))}
        </div>
      </section>
    </div>
  );
}
