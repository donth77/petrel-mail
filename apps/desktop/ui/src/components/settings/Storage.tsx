import { useEffect, useState } from 'react';
import {
  Archive, Download, Inbox, PencilLine, Send, ShieldAlert, Star, Tag as TagIcon, Trash2, Upload,
} from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import { api, type Account, type Folder, type StorageReport, type Tag } from '../../lib/api';
import { fileSize } from '../../lib/format';
import { exportScopes } from '../../lib/export-scopes';
import { Icon } from '../Icon';
import { PickerField, type FieldOption } from '../PickerField';
import { t, type StringId } from '../../lib/strings';
import { useSettings } from '../../lib/settings';

/**
 * The mailboxes worth exporting, in the order someone would think of them.
 *
 * Everything leads, because it is the one that keeps the promise this pane
 * makes: the rest are slices, and a client you can only partly leave is not
 * one you can leave. Sent is in the list for the same reason — mail you wrote
 * was unreachable here until now, which made "your mail stays yours" untrue of
 * the half of it you are the author of.
 *
 * The rail's order, minus Snoozed and the Outbox: both of those are states a
 * message passes through rather than places it lives, and both are already
 * inside Everything. Same order as the sidebar because there is no reason for
 * a second one — a list of mailboxes that disagrees with the list of mailboxes
 * is a thing to re-read twice.
 */
const SCOPES: { view: string; label: StringId }[] = [
  { view: 'inbox', label: 'mailbox-inbox' },
  { view: 'starred', label: 'mailbox-starred' },
  { view: 'sent', label: 'mailbox-sent' },
  { view: 'drafts', label: 'mailbox-drafts' },
  { view: 'archive', label: 'mailbox-archive' },
  { view: 'spam', label: 'mailbox-spam' },
  { view: 'trash', label: 'mailbox-trash' },
];

/** A mailbox's own glyph, so a folder icon is not put on the Outbox. */
const ICONS: Record<string, LucideIcon> = {
  inbox: Inbox,
  archive: Archive,
  sent: Send,
  starred: Star,
  drafts: PencilLine,
  spam: ShieldAlert,
  trash: Trash2,
};

/**
 * One thing that can be exported — a mailbox, a folder, or a tag.
 *
 * Everything is not in here: it is the field's empty choice. "Export all of
 * it" is the absence of a narrowing rather than one more narrowing, which is
 * exactly what the field's none row already means, and spelling it twice would
 * put Everything in the list beside itself.
 */
type Scope = FieldOption & { view: string };

/**
 * A file name from a scope's own words.
 *
 * The view key cannot be used directly any more: `folder:12` says nothing to
 * a person and `tag:read later` is not a filename anyone wants back. The label
 * is what they picked, so the file is named after that.
 */
function scopeSlug(label: string): string {
  const slug = label
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '');
  // Non-Latin labels slug away to nothing — a Japanese tag name has no [a-z0-9]
  // in it at all — and an export called `petrel--me@x.com.mbox` looks broken.
  return slug || 'mail';
}

export function Storage({ onMessage }: { onMessage: (text: string) => void }) {
  const { settings, set } = useSettings();
  const [report, setReport] = useState<StorageReport | null>(null);
  const [accounts, setAccounts] = useState<Account[]>([]);
  // Per account, because the rows are per account: a folder list borrowed
  // from whichever account happens to be on screen would name places that
  // account's export cannot find. Both commands take an account for this.
  const [folders, setFolders] = useState<Record<number, Folder[]>>({});
  const [tags, setTags] = useState<Record<number, Tag[]>>({});
  /** The scope each account's row is set to, keyed by account id. */
  const [chosen, setChosen] = useState<Record<number, string>>({});
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
      .then((a) => {
        if (!live) return;
        setAccounts(a);
        // Quietly, and per account: one with no folders or tags simply offers
        // fewer scopes, and a failure here must not take the mailbox scopes
        // down with it.
        for (const acc of a) {
          api
            .folders(acc.id)
            .then((f) => live && setFolders((prev) => ({ ...prev, [acc.id]: f })))
            .catch(() => {});
          api
            .tags(acc.id)
            .then((x) => live && setTags((prev) => ({ ...prev, [acc.id]: x })))
            .catch(() => {});
        }
      })
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
        defaultPath: `petrel-${scopeSlug(label)}-${account.email}.mbox`,
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
              count: r.imported,
              duplicates: String(r.duplicates),
              failed: String(r.failed),
            })
          : t('storage-imported', { count: r.imported }),
      );
    } catch (e) {
      onMessage(t('storage-import-failed', { error: String(e) }));
    } finally {
      setBusy(false);
    }
  };

  /** The settings backup: preferences and account shapes, never passwords. */
  const exportSettings = async () => {
    setBusy(true);
    try {
      const { save } = await import('@tauri-apps/plugin-dialog');
      const path = await save({
        defaultPath: 'petrel-settings.json',
        filters: [{ name: 'Petrel settings', extensions: ['json'] }],
      });
      if (!path) return;
      const r = await api.exportSettings(path);
      const [prefs, accounts] = r.split('/');
      onMessage(t('settings-exported', { prefs, accounts }));
    } catch (e) {
      onMessage(t('settings-export-failed', { error: String(e) }));
    } finally {
      setBusy(false);
    }
  };

  const importSettings = async () => {
    setBusy(true);
    try {
      const { open } = await import('@tauri-apps/plugin-dialog');
      const picked = await open({
        multiple: false,
        filters: [{ name: 'Petrel settings', extensions: ['json'] }],
      });
      if (!picked || typeof picked !== 'string') return;
      const r = await api.importSettings(picked);
      const [prefs, updated, added] = r.split('/');
      onMessage(t('settings-imported', { prefs, updated, added }));
    } catch (e) {
      onMessage(t('settings-import-failed', { error: String(e) }));
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

  /**
   * Every scope this account can be asked for, in the order they are drawn.
   *
   * The list itself is built in lib/export-scopes, which is where the one
   * interesting decision lives — Archive and Trash are a mailbox *and* a place
   * folders hang under, and they get one row rather than two. Here we only put
   * the glyphs on and number the rows.
   *
   * The id is the row's position rather than anything from the store. Folder
   * ids and tag ids are both row ids and would collide the moment they shared
   * a list; a position cannot.
   */
  const scopesFor = (account: Account): Scope[] =>
    exportScopes(
      SCOPES.map((sc) => ({ view: sc.view, label: t(sc.label) })),
      folders[account.id] ?? [],
      tags[account.id] ?? [],
    ).map((sc, i) => ({
      ...sc,
      id: i,
      icon: ICONS[sc.view] ?? (sc.view.startsWith('tag:') ? TagIcon : undefined),
    }));

  /**
   * One account's scope field and its button.
   *
   * A searchable field rather than a button per scope, and rather than the
   * native list that stood here first: three buttons fitted on a row and three
   * was the whole problem — a mailbox with forty folders and a dozen tags does
   * not. A native popup would type-ahead from the start of the option text,
   * which with full paths means `Archive/Yearly/2023` is reachable only by
   * typing `Archive/Yea…`. This is the control the rules pane already replaced
   * a `<select>` with, for that exact reason.
   */
  const exportControls = (account: Account | undefined) => {
    const scopes = account ? scopesFor(account) : [];
    // Resolved by view key rather than held as an index: the list is rebuilt
    // whenever folders or tags arrive, and an index into the old one would
    // quietly point at a different row. A key that no longer resolves falls
    // back to Everything, which is also what happens if the folder is deleted
    // from under the choice.
    const at = scopes.findIndex((sc) => account && sc.view === chosen[account.id]);
    const scope = at >= 0 ? scopes[at] : null;
    const view = scope?.view ?? 'all';
    const label = scope?.label ?? t('storage-export-everything');
    return (
      <div className="storage-actions">
        <PickerField
          mode="folder"
          label={t('storage-export-scope')}
          value={scope ? scope.id : null}
          options={scopes}
          noneLabel={t('storage-export-everything')}
          onChange={(id) => {
            if (!account) return;
            const picked = id === null ? null : scopes.find((sc) => sc.id === id);
            setChosen((prev) => ({ ...prev, [account.id]: picked?.view ?? 'all' }));
          }}
        />
        <button
          type="button"
          className="fbtn"
          disabled={busy || !account}
          onClick={() => account && void exportTo(account, view, label)}
        >
          <Icon icon={Download} size={13} />
          {t('storage-export-button')}
        </button>
      </div>
    );
  };

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
              {exportControls(a)}
            </div>
          ))
        ) : (
          exportControls(accounts[0])
        )}
      </section>

      <section className="field">
        <div className="flabel">{t('trash-retention')}</div>
        <p className="fhelp">{t('trash-retention-help')}</p>
        <select
          className="select"
          value={settings.trashRetentionDays}
          onChange={(e) => set('trashRetentionDays', e.target.value as '0' | '7' | '30' | '90')}
        >
          <option value="0">{t('trash-retention-off')}</option>
          <option value="7">{t('trash-retention-days', { days: '7' })}</option>
          <option value="30">{t('trash-retention-days', { days: '30' })}</option>
          <option value="90">{t('trash-retention-days', { days: '90' })}</option>
        </select>
        {settings.trashRetentionDays !== '0' && (
          <p className="fhelp">{t('trash-retention-on-note')}</p>
        )}
      </section>

      <section className="field">
        <div className="flabel">{t('settings-backup')}</div>
        <p className="fhelp">{t('settings-backup-help')}</p>
        <div className="storage-actions">
          <button type="button" className="fbtn" disabled={busy} onClick={() => void exportSettings()}>
            <Icon icon={Download} size={13} />
            {t('settings-export-button')}
          </button>
          <button type="button" className="fbtn" disabled={busy} onClick={() => void importSettings()}>
            <Icon icon={Upload} size={13} />
            {t('settings-import-button')}
          </button>
        </div>
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
