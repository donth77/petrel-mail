import type React from 'react';
import { useState } from 'react';
import { Menu, MenuButton, MenuItem, MenuProvider, MenuSeparator } from '@ariakit/react';
import {
  FolderInput,
  FolderPlus,
  FolderX,
  Mail,
  MailOpen,
  MoreHorizontal,
  SquarePen,
  Trash2,
} from 'lucide-react';
import { Icon } from './Icon';
import { t } from '../lib/strings';

/**
 * Renaming and deleting one folder, from the rail row it lives on.
 *
 * The same shape as the tag menu and for the same reason: the rail is where
 * you are when you notice the name is wrong. No colours — a folder is a place
 * on the server, not a label, and inventing local paint for it would suggest
 * a property the server does not have.
 */
export function FolderMenu({
  path,
  onRename,
  onNewChild,
  onEmpty,
  onMove,
  onDelete,
  onMarkAll,
  onTrashAll,
}: {
  path: string;
  /** Absent on rows that are not renameable — the Archive root is the
   *  archive mailbox wearing its tree, not a folder anyone named. */
  onRename?: () => void;
  /** Opens the naming field prefilled with this folder's path — a subfolder
   *  is a name with a parent already decided. */
  /** Absent on a folder that takes no children — the bin. */
  onNewChild?: () => void;
  /** The bin's own verb, and the only irreversible one in the app. */
  onEmpty?: () => void;
  /** Opens a destination picker — the menu's answer to the drag. */
  onMove?: () => void;
  /** Re-nests under Archive — present on folders standing outside it. */
  /** Pulls back to the top level — present on folders inside Archive. */
  onDelete?: () => void;
  /** Everything in the folder, read or unread in one go. Absent where there is
   *  nothing to mark — the Outbox, and a rail row with no folder behind it. */
  onMarkAll?: (read: boolean) => void;
  /** Everything in the folder to the Trash. The folder itself stays. Absent on
   *  the Trash, where Empty Trash is the stronger verb and two of them on one
   *  menu is a question nobody should have to answer. */
  onTrashAll?: () => void;
}) {
  const [open, setOpen] = useState(false);
  return (
    <MenuProvider open={open} setOpen={setOpen} placement="bottom-end">
      <MenuButton
        className="tag-edit"
        aria-label={t('folder-edit')}
        // The rail row is itself a button, and a press here must not also
        // switch the view to the folder being edited.
        onClick={(e: React.MouseEvent) => e.stopPropagation()}
        onPointerDown={(e: React.PointerEvent) => e.stopPropagation()}
      >
        <Icon icon={MoreHorizontal} size={14} />
      </MenuButton>
      <Menu portal gutter={6} className="menu" aria-label={t('folder-edit', { name: path })}>
        {onRename && (
          <MenuItem className="menu-item" onClick={onRename}>
            <Icon icon={SquarePen} size={14} />
            <span className="menu-label">{t('folder-rename')}</span>
          </MenuItem>
        )}
        {onNewChild && (
          <MenuItem className="menu-item" onClick={onNewChild}>
            <Icon icon={FolderPlus} size={14} />
            <span className="menu-label">{t('folder-subfolder')}</span>
          </MenuItem>
        )}
        {onEmpty && (
          <MenuItem className="menu-item danger" onClick={onEmpty}>
            <Icon icon={Trash2} size={14} />
            <span className="menu-label">{t('trash-empty')}</span>
          </MenuItem>
        )}
        {onMove && (
          <MenuItem className="menu-item" onClick={onMove}>
            <Icon icon={FolderInput} size={14} />
            <span className="menu-label">{t('folder-move')}</span>
          </MenuItem>
        )}
        {/* The three that act on the mail rather than on the folder, kept
            together and away from Rename and Move so a slip between the two
            groups cannot happen. Trashing everything is destructive enough to
            wear the danger colour and to ask first. */}
        {onMarkAll && (
          <>
            {/* Only where there is something above it to divide from. On the
                Inbox these three are the whole menu, and a rule across the top
                of it separates nothing from nothing. */}
            {(onRename || onNewChild || onEmpty || onMove) && (
              <MenuSeparator className="menu-sep" />
            )}
            <MenuItem className="menu-item" onClick={() => onMarkAll(true)}>
              <Icon icon={MailOpen} size={14} />
              <span className="menu-label">{t('folder-mark-all-read')}</span>
            </MenuItem>
            <MenuItem className="menu-item" onClick={() => onMarkAll(false)}>
              <Icon icon={Mail} size={14} />
              <span className="menu-label">{t('folder-mark-all-unread')}</span>
            </MenuItem>
          </>
        )}
        {onTrashAll && (
          <MenuItem className="menu-item danger" onClick={onTrashAll}>
            <Icon icon={Trash2} size={14} />
            <span className="menu-label">{t('folder-trash-all')}</span>
          </MenuItem>
        )}
        {onDelete && (
          <>
            <MenuSeparator className="menu-sep" />
            <MenuItem className="menu-item danger" onClick={onDelete}>
              {/* FolderX rather than a bin: this one takes the folder itself,
                  which is a different thing from the two above that take what
                  is in it. */}
              <Icon icon={FolderX} size={14} />
              <span className="menu-label">{t('folder-delete')}</span>
            </MenuItem>
          </>
        )}
      </Menu>
    </MenuProvider>
  );
}
