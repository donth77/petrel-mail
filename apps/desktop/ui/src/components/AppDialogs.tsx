import type { Dispatch, SetStateAction } from 'react';
import { api, type ActionKind, type Folder, type OutboxRow, type Thread } from '../lib/api';
import { count as fmtCount } from '../lib/format';
import { t } from '../lib/strings';
import {
  binDestination,
  binTakesFolders,
  foldersAreLabels,
  folderDelimiter,
  folderLeaf,
  movedFolderPath,
  nameIsTaken,
  nestableRolePath,
  underAnchor,
} from '../lib/folders';
import { Archive as ArchiveIcon, Trash2 } from 'lucide-react';
import { MAILBOX_LOOK, type MailboxKey } from '../lib/mailboxes';
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
  trashingAll,
  setTrashingAll,
  onTrashedAll,
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
  emptyingTrash,
  onCancelEmptyTrash,
  onEmptyTrash,
}: {
  discarding: OutboxRow | null;
  setDiscarding: Dispatch<SetStateAction<OutboxRow | null>>;
  deletingTag: { id: number; name: string } | null;
  setDeletingTag: Dispatch<SetStateAction<{ id: number; name: string } | null>>;
  movingFolder: Folder | null;
  setMovingFolder: Dispatch<SetStateAction<Folder | null>>;
  deletingFolder: Folder | null;
  trashingAll: { folder: Folder; count: number } | null;
  setTrashingAll: Dispatch<SetStateAction<{ folder: Folder; count: number } | null>>;
  onTrashedAll: () => void;
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
  emptyingTrash: boolean;
  onCancelEmptyTrash: () => void;
  onEmptyTrash: () => void;
}) {
  /** What to call a folder in a sentence.
   *
   *  A role folder wears the name the rail gives it, not the one the server
   *  does: "Move everything in INBOX to the Trash?" is a question about a
   *  protocol, and the row it came from says Inbox. Everything else is its own
   *  leaf, read with the delimiter this account actually uses. */
  const folderName = (f: Folder | null | undefined) => {
    if (!f) return '';
    const look = MAILBOX_LOOK[f.role as MailboxKey];
    return look ? t(look.label) : folderLeaf(f.path, folderDelimiter(folders));
  };

  /** Whether a folder already sits in the bin, which is what makes deleting
   *  it mean deletion rather than a move. */
  const binned = (f: Folder | null) =>
    f !== null && underAnchor(f.path, nestableRolePath(folders, 'trash'));

  /** Where Delete would put this folder, or nothing when Delete means delete.
   *
   *  One answer for the wording and the act, because they must not disagree.
   *  An account with no trash folder at all has nowhere to move to, and the
   *  dialog used to offer "Move to Trash" and then delete the folder outright
   *  — the one place in the app where a confirmation could say the opposite
   *  of what the button did. */
  const binFor = (f: Folder | null) =>
    f !== null && !binned(f) ? binDestination(folders, f) : undefined;

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
          // Not offered where the bin cannot hold a folder — Gmail's takes no
          // children, and a destination that silently is not one is worse
          // than a shorter list. Delete is still on the folder's own menu.
          if (
            trash &&
            trashAnchor &&
            binTakesFolders(folders) &&
            !within(movingFolder.path, trashAnchor)
          ) {
            out.push({ id: trash.id, label: t('mailbox-trash'), icon: Trash2 });
          }
          return out;
        })()}
        onClose={() => setMovingFolder(null)}
        onChoose={(id) => {
          const f = movingFolder;
          setMovingFolder(null);
          if (!f) return;
          const leaf = folderLeaf(f.path, folderDelimiter(folders));
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
          // The bin numbers a name it already holds; every other destination
          // keeps the name and says so when the server refuses.
          const next =
            chosen?.role === 'trash'
              ? binDestination(folders, f)
              : movedFolderPath(folders, f, targetPath);
          if (!next || next === f.path) return;
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
            .catch((e) =>
              setToast(
                nameIsTaken(e)
                  ? t('folder-name-taken', { name: leaf })
                  : t('folder-failed', { error: String(e) }),
              ),
            );
        }}
        onCreate={() => {}}
      />

      {/* Deleting a folder puts it in the Trash first, exactly as dragging
          it there does — one word, one meaning. Only a folder already in the
          Trash is deleted outright, which is also the only time the wording
          promises that. */}
      <Confirm
        open={deletingFolder !== null}
        title={
          binFor(deletingFolder)
            ? t('folder-trash-confirm', { name: deletingFolder?.path ?? '' })
            : t('folder-delete-confirm', { name: deletingFolder?.path ?? '' })
        }
        detail={
          binFor(deletingFolder)
            ? t('folder-trash-body')
            : // What deleting costs is not the same on both kinds of account,
              // and this is the sentence somebody reads before an action they
              // cannot take back. On a plain server the folder's mail goes
              // with it; on Gmail a label comes off and the mail stays, in
              // All Mail and under whatever else it carries. Saying the first
              // on an account that means the second is how a safe action gets
              // a frightening dialog — and would have been a plain lie now
              // that Gmail has no other way to delete a folder.
              t(foldersAreLabels(folders) ? 'folder-delete-body-label' : 'folder-delete-body')
        }
        confirmLabel={binFor(deletingFolder) ? t('folder-trash-do') : t('folder-delete')}
        onClose={() => setDeletingFolder(null)}
        onConfirm={() => {
          const folder = deletingFolder;
          setDeletingFolder(null);
          if (!folder) return;
          if (view === `folder:${folder.id}`) setView('inbox');
          const leaf = folderLeaf(folder.path, folderDelimiter(folders));
          // The same answer the wording was written from — numbered when the
          // bin already holds that name, because the server refuses a RENAME
          // onto an occupied one and the folder would simply stay put.
          const bin = binFor(folder);
          const act = bin
            ? api.renameFolder(folder.id, bin).then(() => t('folder-trashed', { name: leaf }))
            : api.deleteFolder(folder.id).then(() => t('folder-deleted', { name: folder.path }));
          void act
            .then((message) => api.folders().then(setFolders).then(() => message))
            .then((message) => setToast(message))
            .catch((e) =>
              setToast(
                nameIsTaken(e)
                  ? t('folder-name-taken', { name: leaf })
                  : t('folder-failed', { error: String(e) }),
              ),
            );
        }}
      />

      {/* Everything in a folder to the Trash. Asks, and asks with the number
          in it: "move everything" is a different decision at four messages and
          at ten thousand, and the menu item cannot know which one you meant.
          Not a delete-forever — the mail is in the Trash and comes back out —
          so it wears the same wording as any other binning. */}
      <Confirm
        open={trashingAll !== null}
        title={t('folder-trash-all-confirm', { name: folderName(trashingAll?.folder) })}
        detail={t('folder-trash-all-body', { count: fmtCount(trashingAll?.count ?? 0) })}
        confirmLabel={t('folder-trash-all-do')}
        onClose={() => setTrashingAll(null)}
        onConfirm={() => {
          const target = trashingAll;
          setTrashingAll(null);
          if (!target) return;
          void api
            .trashFolderContents(target.folder.id)
            .then((n) => {
              setToast(t('folder-trashed-all', { count: fmtCount(n) }));
              onTrashedAll();
            })
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
      <Confirm
        open={emptyingTrash}
        title={t('trash-empty-confirm')}
        detail={t('trash-empty-body')}
        confirmLabel={t('trash-empty')}
        onClose={onCancelEmptyTrash}
        onConfirm={onEmptyTrash}
      />
    </>
  );
}
