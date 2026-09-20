import type React from 'react';
import { Menu, MenuButton, MenuItem, MenuProvider, MenuSeparator } from '@ariakit/react';
import { ChevronDown, ChevronUp, MoreHorizontal, SquarePen, Trash2 } from 'lucide-react';
import { Icon } from './Icon';
import { t } from '../lib/strings';

/**
 * Renaming and deleting one saved search, from the rail row it lives on.
 *
 * The same shape as the folder and tag menus, and deliberately shorter: a
 * saved search has no colour to set and nowhere to be moved to. Its query is
 * changed by opening it and editing the field, which is where a query lives —
 * not here, in a menu that would be a second place to edit one.
 */
export function SearchMenu({
  name,
  first,
  last,
  onReorder,
  onRename,
  onDelete,
}: {
  name: string;
  /** Hidden at the ends rather than shown inert: a menu is built afresh each
   *  time it opens, so nothing loses focus when an item is absent, and an item
   *  that cannot do anything is one more thing to read past. */
  first: boolean;
  last: boolean;
  onReorder: (up: boolean) => void;
  onRename: () => void;
  onDelete: () => void;
}) {
  return (
    <MenuProvider>
      <MenuButton
        className="tag-edit"
        aria-label={t('search-menu', { name })}
        // The row behind this is a button that navigates, and the rail's drag
        // starter treats a nested control as a control rather than a handle.
        onClick={(e: React.MouseEvent) => e.stopPropagation()}
        onPointerDown={(e: React.PointerEvent) => e.stopPropagation()}
      >
        <Icon icon={MoreHorizontal} size={13} />
      </MenuButton>
      <Menu
        gutter={4}
        className="menu"
        portal
        // As in the tag and folder menus: a portalled menu bubbles through the
        // React tree, so without this choosing Rename searched for the thing
        // being renamed.
        onClick={(e: React.MouseEvent) => e.stopPropagation()}
      >
        {/* Reordering from the keyboard. Dragging is the quicker gesture and the
            only one people find, but a rail that can only be arranged with a
            pointer is a rail some people cannot arrange — the same reason the
            settings pane arranges mailboxes with buttons. */}
        {!first && (
          <MenuItem className="menu-item" onClick={() => onReorder(true)}>
            <Icon icon={ChevronUp} size={13} />
            <span>{t('move-up')}</span>
          </MenuItem>
        )}
        {!last && (
          <MenuItem className="menu-item" onClick={() => onReorder(false)}>
            <Icon icon={ChevronDown} size={13} />
            <span>{t('move-down')}</span>
          </MenuItem>
        )}
        {(!first || !last) && <MenuSeparator className="menu-sep" />}
        <MenuItem className="menu-item" onClick={onRename}>
          <Icon icon={SquarePen} size={13} />
          <span>{t('search-rename')}</span>
        </MenuItem>
        <MenuItem className="menu-item danger" onClick={onDelete}>
          <Icon icon={Trash2} size={13} />
          <span>{t('search-delete')}</span>
        </MenuItem>
      </Menu>
    </MenuProvider>
  );
}
