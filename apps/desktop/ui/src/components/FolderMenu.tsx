import type React from 'react';
import { useState } from 'react';
import { Menu, MenuButton, MenuItem, MenuProvider, MenuSeparator } from '@ariakit/react';
import { MoreHorizontal } from 'lucide-react';
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
  onDelete,
}: {
  path: string;
  onRename: () => void;
  onDelete: () => void;
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
        <MenuItem className="menu-item" onClick={onRename}>
          {t('folder-rename')}
        </MenuItem>
        <MenuSeparator className="menu-sep" />
        <MenuItem className="menu-item danger" onClick={onDelete}>
          {t('folder-delete')}
        </MenuItem>
      </Menu>
    </MenuProvider>
  );
}
