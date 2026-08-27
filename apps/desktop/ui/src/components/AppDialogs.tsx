import type { Dispatch, SetStateAction } from 'react';
import { api, type ActionKind, type Folder, type OutboxRow, type Thread } from '../lib/api';
import { count as fmtCount } from '../lib/format';
import { t } from '../lib/strings';
import { nestableRolePath } from '../lib/folders';
import { Archive as ArchiveIcon, Trash2 } from 'lucide-react';
import { Confirm } from './Confirm';
import { Dialog } from '@ariakit/react';
import { Picker, type PickerOption } from './Picker';

/**
 * The app's confirmation stack: every dialog that stands between a click
 * and something that is hard or impossible to take back.
 *
 * Lifted out of App verbatim — markup and handlers together, because a
 * confirm's meaning *is* its handler, and splitting them across files
 * would make either half unreadable. App owns the state; this owns what
 * the dialogs say and do with it.
 */
export function AppDialogs({
  discarding,
  setDiscarding,
  deletingTag,
  setDeletingTag,
  movingFolder,
  setMovingFolder,
  deletingFolder,
  setDeletingFolder,
  pendingDelete,
  setPendingDelete,
  view,
  setView,
  folders,
  setFolders,
  setTags,
  setToast,
  items,
  selectedSize,
  clearSelected,
  runTriage,
  clearUndo,
  draftConflict,
  onSettleDraftConflict,
  riskyLink,
  onDismissRiskyLink,
}: {
  discarding: OutboxRow | null;
  setDiscarding: Dispatch<SetStateAction<OutboxRow | null>>;
  deletingTag: { id: number; name: string } | null;
  setDeletingTag: Dispatch<SetStateAction<{ id: number; name: string } | null>>;
  movingFolder: Folder | null;
  setMovingFolder: Dispatch<SetStateAction<Folder | null>>;
  deletingFolder: Folder | null;
  setDeletingFolder: Dispatch<SetStateAction<Folder | null>>;
  pendingDelete: number[] | null;
  setPendingDelete: Dispatch<SetStateAction<number[] | null>>;
  view: string;
  setView: (v: string) => void;
  folders: Folder[];
  setFolders: (f: Folder[]) => void;
  setTags: (tags: import('../lib/api').Tag[]) => void;
  setToast: (text: string | null) => void;
  items: Thread[];
  selectedSize: number;
  clearSelected: () => void;
  runTriage: (kind: ActionKind, threadId?: number, targetId?: number, quiet?: boolean) => void;
  clearUndo: () => void;
  draftConflict: { draftId: number; otherId: number } | null;
  onSettleDraftConflict: (takeServer: boolean) => void;
  riskyLink: { risk: import('../lib/links').HomographRisk; open: () => void } | null;
  onDismissRiskyLink: () => void;
}) {
  return (
    <>
      <Confirm
        open={discarding !== null}
        title={t('outbox-discard-confirm', { subject: discarding?.subject || t('no-subject') })}
        detail={t('outbox-discard-body')}
        confirmLabel={t('outbox-discard')}
        onClose={() => setDiscarding(null)}
        onConfirm={() => {
          const row = discarding;
          setDiscarding(null);
          if (!row) return;
          void api
            .deleteDraft(row.id)
            .catch((e) => setToast(t('triage-failed', { error: String(e) })));
        }}
      />

      <Confirm
        open={deletingTag !== null}
        title={t('tag-delete-confirm', { name: deletingTag?.name ?? '' })}
        detail={t('tag-delete-body')}
        confirmLabel={t('tag-delete')}
        onClose={() => setDeletingTag(null)}
        onConfirm={() => {
          const tag = deletingTag;
          setDeletingTag(null);
          if (!tag) return;
          // Leaving the tag's own view would strand the user looking at a list
          // that can no longer exist.
          if (view === `tag:${tag.name}`) setView('inbox');
          void api
            .deleteTag(tag.id)
            .then(() => api.tags().then(setTags))
            .then(() => setToast(t('tag-deleted', { name: tag.name })))
            .catch((e) => setToast(t('tag-rename-failed', { error: String(e) })));
        }}
      />

      <Picker
        open={movingFolder !== null}
        mode="folder"
        subject={movingFolder ? (movingFolder.path.split(/[/.]/).pop() ?? movingFolder.path) : null}
        options={(() => {
          if (!movingFolder) return [];
          const archiveAnchor = nestableRolePath(folders, 'archive');
          const trashAnchor = nestableRolePath(folders, 'trash');
          const within = (p: string, root: string | undefined) =>
            root !== undefined && (p === root || p.startsWith(`${root}/`) || p.startsWith(`${root}.`));
          // Where the folder already stands is not a move: a top-level folder
          // is not offered Top level, and a child is not offered its own
          // parent.
          const parent = movingFolder.path.includes('/')
            ? movingFolder.path.slice(0, movingFolder.path.lastIndexOf('/'))
            : null;
          const out: PickerOption[] = parent === null ? [] : [{ id: -1, label: t('folder-move-top') }];
          for (const f of folders) {
            if (f.role) continue;
            if (f.id === movingFolder.id) continue;
            if (f.path === movingFolder.path || f.path.startsWith(`${movingFolder.path}/`))
              continue;
            if (f.path === parent) continue;
            // The labels wearing the anchors' own names are the pinned rows'
            // business, and nothing nests into a binned folder — restoring
            // one is its own Move.
            if (f.path === archiveAnchor || f.path === trashAnchor) continue;
            if (within(f.path, trashAnchor)) continue;
            out.push({ id: f.id, label: f.path });
          }
          // Archive and Trash close the list, each wearing its own glyph —
          // and never offered to a folder already standing in it. The row
          // behind the option is whichever folder holds the anchor: the role
          // folder where one exists, or the plain folder doing the job by
          // name — Namecheap marks no \Archive at all.
          const anchorRow = (anchor: string | undefined, role: string) =>
            folders.find((f) => f.role === role) ??
            folders.find((f) => !f.role && f.path === anchor);
          const archive = anchorRow(archiveAnchor, 'archive');
          if (archive && archiveAnchor && !within(movingFolder.path, archiveAnchor)) {
            out.push({ id: archive.id, label: t('mailbox-archive'), icon: ArchiveIcon });
          }
          const trash = anchorRow(trashAnchor, 'trash');
          if (trash && trashAnchor && !within(movingFolder.path, trashAnchor)) {
            out.push({ id: trash.id, label: t('mailbox-trash'), icon: Trash2 });
          }
          return out;
        })()}
        onClose={() => setMovingFolder(null)}
        onChoose={(id) => {
          const f = movingFolder;
          setMovingFolder(null);
          if (!f) return;
          const leaf = f.path.split(/[/.]/).pop() ?? f.path;
          // The pinned rows carry the role folders' ids, but the rename goes
          // to the nestable anchor — on Gmail an ordinary Archive or Trash
          // label, never the reserved [Gmail] names.
          const chosen = folders.find((x) => x.id === id);
          const targetPath =
            id === -1
              ? ''
              : chosen?.role === 'archive' || chosen?.role === 'trash'
                ? (nestableRolePath(folders, chosen.role) ?? '')
                : (chosen?.path ?? '');
          const next = targetPath ? `${targetPath}/${leaf}` : leaf;
          if (next === f.path) return;
          void api
            .renameFolder(f.id, next)
            .then(() => api.folders().then(setFolders))
            .then(() =>
              setToast(
                chosen?.role === 'trash'
                  ? t('folder-trashed', { name: leaf })
                  : t('folder-moved', { name: leaf, to: targetPath || t('rail-folders') }),
              ),
            )
            .catch((e) => setToast(t('folder-failed', { error: String(e) })));
        }}
        onCreate={() => {}}
      />

      <Confirm
        open={deletingFolder !== null}
        title={t('folder-delete-confirm', { name: deletingFolder?.path ?? '' })}
        detail={t('folder-delete-body')}
        confirmLabel={t('folder-delete')}
        onClose={() => setDeletingFolder(null)}
        onConfirm={() => {
          const folder = deletingFolder;
          setDeletingFolder(null);
          if (!folder) return;
          if (view === `folder:${folder.id}`) setView('inbox');
          void api
            .deleteFolder(folder.id)
            .then(() => api.folders().then(setFolders))
            .then(() => setToast(t('folder-deleted', { name: folder.path })))
            .catch((e) => setToast(t('folder-failed', { error: String(e) })));
        }}
      />

      <Confirm
        open={pendingDelete !== null}
        title={t('delete-forever-confirm')}
        detail={
          pendingDelete?.length === 1
            ? t('delete-forever-one', {
                subject:
                  // Either kind of id can be in here — the keyboard path
                  // carries the active message id and the row path a thread
                  // id — so match the way triage itself does.
                  items.find(
                    (m) => m.id === pendingDelete[0] || m.thread_id === pendingDelete[0],
                  )?.subject || t('no-subject'),
              })
            : t('delete-forever-many', { count: fmtCount(pendingDelete?.length ?? 0) })
        }
        confirmLabel={t('delete-forever')}
        onClose={() => setPendingDelete(null)}
        onConfirm={() => {
          const ids = pendingDelete ?? [];
          setPendingDelete(null);
          // No undo offered, because there is none to offer. The toast reports
          // what happened and stops there.
          ids.forEach((id) => runTriage('delete_forever', id, undefined, true));
          if (selectedSize > 0) clearSelected();
          // Clear the standing offer before saying anything. The toast is one
          // surface: leaving the previous action's Undo attached to this
          // message puts an undo button on a permanent delete, which is the
          // precise lie the confirmation dialog exists to avoid.
          clearUndo();
          setToast(t('deleted-forever'));
        }}
      />

      {/* A draft with two living versions: ours, and one another client
          saved. Neither was discarded — that is what makes this a question
          rather than a loss — and dismissing chooses nothing: the composer
          holds the local words until one button says otherwise. */}
      <Dialog
        open={draftConflict !== null}
        onClose={() => onSettleDraftConflict(false)}
        className="confirm-backdrop"
        backdrop={<div className="palette-scrim" />}
        aria-label={t('draft-conflict-title')}
      >
        <div className="confirm" role="alertdialog">
          <div className="confirm-title">{t('draft-conflict-title')}</div>
          <p className="confirm-detail">{t('draft-conflict-body')}</p>
          <div className="confirm-foot">
            <button type="button" className="reply" onClick={() => onSettleDraftConflict(false)}>
              {t('draft-keep-local')}
            </button>
            <button
              type="button"
              className="reply primary"
              onClick={() => onSettleDraftConflict(true)}
            >
              {t('draft-take-server')}
            </button>
          </div>
        </div>
      </Dialog>
      {/* A link that reads as one address and resolves to another. Both
          spellings are shown, and the safe answer is the default: doing
          nothing leaves the browser unopened. */}
      <Dialog
        open={riskyLink !== null}
        onClose={onDismissRiskyLink}
        className="confirm-backdrop"
        backdrop={<div className="palette-scrim" />}
        aria-label={t('link-risk-title')}
      >
        <div className="confirm" role="alertdialog">
          <div className="confirm-title">{t('link-risk-title')}</div>
          <p className="confirm-detail">
            {t('link-risk-body', {
              typed: riskyLink?.risk.asTyped ?? '',
              real: riskyLink?.risk.asPunycode ?? '',
            })}
          </p>
          <div className="confirm-foot">
            <button type="button" className="reply" onClick={onDismissRiskyLink}>
              {t('link-risk-stay')}
            </button>
            <button
              type="button"
              className="reply danger"
              onClick={() => {
                riskyLink?.open();
                onDismissRiskyLink();
              }}
            >
              {t('link-risk-open')}
            </button>
          </div>
        </div>
      </Dialog>
    </>
  );
}
