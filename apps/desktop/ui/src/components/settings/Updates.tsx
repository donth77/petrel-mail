import { useEffect, useState } from 'react';
import { Download, RefreshCw, RotateCw } from 'lucide-react';
import { api, type UpdateStatus } from '../../lib/api';
import { Icon } from '../Icon';
import { t, type StringId } from '../../lib/strings';

/** A category from the engine becomes a sentence here, where it can be
 *  translated. Anything unrecognised falls to the vague one rather than
 *  showing a code. */
const ERROR_TEXT: Record<string, StringId> = {
  offline: 'update-err-offline',
  'not-configured': 'update-err-not-configured',
  missing: 'update-err-missing',
  malformed: 'update-err-malformed',
  unknown: 'update-err-unknown',
};
const errorText = (kind: string) => t(ERROR_TEXT[kind] ?? 'update-err-unknown');

/**
 * Updates, asked for rather than arriving.
 *
 * Checking is a button, not something the app does at launch: an updater
 * that phones home on its own is a second network dependency between a
 * person and their mail, and one that can replace the running program.
 * Installing and restarting are separate presses for the same reason — an
 * app that restarts itself while a reply is half-written is worse than one
 * that waits to be asked.
 */
export function Updates({ onMessage }: { onMessage: (text: string) => void }) {
  const [status, setStatus] = useState<UpdateStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [installed, setInstalled] = useState(false);

  // The version is a fact about this app and costs nothing to read, so the
  // pane can say what it is before anyone asks it to look further.
  useEffect(() => {
    let live = true;
    api
      .checkUpdate()
      .then((s) => live && setStatus(s))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, []);

  const check = async () => {
    setBusy(true);
    try {
      const s = await api.checkUpdate();
      setStatus(s);
      onMessage(
        s.error
          ? errorText(s.error)
          : s.available
            ? t('update-found', { version: s.available })
            : t('update-none'),
      );
    } catch (e) {
      // The command itself failing, as opposed to reporting an error status:
      // no network, or the IPC call refused. `try/finally` alone cleared the
      // busy flag and let the rejection go nowhere, so pressing Check while
      // offline was indistinguishable from pressing a dead button — and the
      // string written for exactly this had never been wired to anything.
      onMessage(t('update-check-failed', { error: String(e) }));
    } finally {
      setBusy(false);
    }
  };

  const install = async () => {
    setBusy(true);
    try {
      await api.installUpdate();
      setInstalled(true);
      onMessage(t('update-installed'));
    } catch (e) {
      onMessage(t('update-install-failed', { error: String(e) }));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="pane-body">
      <h1 className="pane-title">{t('settings-updates')}</h1>

      <section className="field">
        <div className="flabel">{t('update-this-version')}</div>
        <p className="fhelp">
          {status ? t('update-running', { version: status.current }) : t('update-reading')}
        </p>
        {/* What changed in the build you are running. Compiled in rather than
            fetched, so it is here offline and cannot be a spinner. A dev build
            has none, and shows none. The box is the same fixed, scrollable one
            an available update's notes get — long notes must not stretch the
            pane. */}
        {status?.current_notes && (
          <p className="fhelp update-notes">{status.current_notes}</p>
        )}
        <div className="storage-actions">
          <button type="button" className="fbtn" disabled={busy} onClick={() => void check()}>
            <Icon icon={RefreshCw} size={13} />
            {t('update-check')}
          </button>
        </div>
      </section>

      {status?.error && (
        <section className="field">
          <div className="flabel">{t('update-could-not-ask')}</div>
          {/* Said plainly rather than shown as "up to date": not knowing and
              knowing there is nothing are different answers. */}
          <p className="fhelp">{errorText(status.error)}</p>
        </section>
      )}

      {status?.available && !installed && (
        <section className="field">
          <div className="flabel">{t('update-available', { version: status.available })}</div>
          <p className="fhelp">{t('update-signed-note')}</p>
          {status.notes && <p className="fhelp update-notes">{status.notes}</p>}
          <div className="storage-actions">
            <button type="button" className="fbtn" disabled={busy} onClick={() => void install()}>
              <Icon icon={Download} size={13} />
              {t('update-install')}
            </button>
          </div>
        </section>
      )}

      {installed && (
        <section className="field">
          <div className="flabel">{t('update-ready')}</div>
          <p className="fhelp">{t('update-restart-note')}</p>
          <div className="storage-actions">
            <button type="button" className="fbtn" onClick={() => void api.restartForUpdate()}>
              <Icon icon={RotateCw} size={13} />
              {t('update-restart')}
            </button>
          </div>
        </section>
      )}
    </div>
  );
}
