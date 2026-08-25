import type { Dispatch, SetStateAction } from 'react';
import { api, type ActionKind, type Folder, type OutboxRow, type Thread } from '../lib/api';
import { count as fmtCount } from '../lib/format';
import { t } from '../lib/strings';
import { Confirm } from './Confirm';
import { Picker } from './Picker';

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
          const out = [{ id: -1, label: t('folder-move-top') }];
          const archive = folders.find((f) => f.role === 'archive');
          if (archive && movingFolder.path !== archive.path) {
            out.push({ id: archive.id, label: 'Archive' });
          }
          for (const f of folders) {
            if (f.role) continue;
            if (f.id === movingFolder.id) continue;
            if (f.path === movingFolder.path || f.path.startsWith(`${movingFolder.path}/`))
              continue;
            out.push({ id: f.id, label: f.path });
          }
          return out;
        })()}
        onClose={() => setMovingFolder(null)}
        onChoose={(id) => {
          const f = movingFolder;
          setMovingFolder(null);
          if (!f) return;
          const leaf = f.path.split(/[/.]/).pop() ?? f.path;
          const targetPath = id === -1 ? '' : (folders.find((x) => x.id === id)?.path ?? '');
          const next = targetPath ? `${targetPath}/${leaf}` : leaf;
          if (next === f.path) return;
          void api
            .renameFolder(f.id, next)
            .then(() => api.folders().then(setFolders))
            .then(() =>
              setToast(t('folder-moved', { name: leaf, to: targetPath || t('rail-folders') })),
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

    </>
  );
}
