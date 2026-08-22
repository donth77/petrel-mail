import {
  Menu, MenuButton, MenuItem, MenuProvider, MenuSeparator,
} from '@ariakit/react';
import {
  Archive, Clock, FolderClosed, Mail, MailOpen, MoreVertical, ShieldAlert, Star,
  Tag as TagIcon, Trash2,
} from 'lucide-react';
import type { ActionKind, Thread } from '../lib/api';
import { Icon } from './Icon';
import { key } from '../lib/keys';
import { t } from '../lib/strings';
import { Tip } from './Tip';

type Props = {
  thread: Thread;
  /** Which view is open. In the trash there is nowhere left to move something
   *  to, so the destructive item becomes the permanent one. */
  view: string;
  onAction: (kind: ActionKind) => void;
  onMove: () => void;
  onTag: () => void;
  onSnooze: () => void;
};

/**
 * The overflow menu behind the ⋮ button.
 *
 * It used to open the command palette. That was a shortcut — the palette does
 * contain these commands — but it reads as a bug: a button anchored to one
 * conversation opening a full-screen searchable launcher is not what ⋮ means
 * anywhere else, and it loses the anchoring that tells you *which* conversation
 * you are acting on. The palette is still there on its own shortcut, for when
 * you want to search rather than point.
 *
 * Every item here has a keyboard equivalent, shown alongside — the menu is for
 * discovering them, not a substitute for learning them.
 */
export function MoreMenu({ thread, view, onAction, onMove, onTag, onSnooze }: Props) {
  const inTrash = view === 'trash';
  return (
    <MenuProvider placement="bottom-end">
      <Tip label={t('reader-more')}>
        <MenuButton className="act-icon" aria-label={t('reader-more')}>
          <Icon icon={MoreVertical} />
        </MenuButton>
      </Tip>
      <Menu portal gutter={6} className="menu" aria-label={t('reader-more')}>
        <MenuItem className="menu-item" onClick={() => onAction(thread.starred ? 'unstar' : 'star')}>
          <Icon icon={Star} size={14} />
          <span className="menu-label">{thread.starred ? t('menu-unstar') : t('menu-star')}</span>
          <span className="menu-key">S</span>
        </MenuItem>
        <MenuItem
          className="menu-item"
          onClick={() => onAction(thread.unread ? 'mark_read' : 'mark_unread')}
        >
          <Icon icon={thread.unread ? MailOpen : Mail} size={14} />
          <span className="menu-label">{thread.unread ? t('reader-mark-read') : t('reader-mark-unread')}</span>
          <span className="menu-key">{thread.unread ? key('read') : key('unread')}</span>
        </MenuItem>

        <MenuSeparator className="menu-sep" />

        <MenuItem className="menu-item" onClick={onMove}>
          <Icon icon={FolderClosed} size={14} />
          <span className="menu-label">{t('picker-folder-title')}</span>
          <span className="menu-key">V</span>
        </MenuItem>
        <MenuItem className="menu-item" onClick={onTag}>
          <Icon icon={TagIcon} size={14} />
          <span className="menu-label">{t('picker-tag-title')}</span>
          <span className="menu-key">L</span>
        </MenuItem>
        <MenuItem className="menu-item" onClick={onSnooze}>
          <Icon icon={Clock} size={14} />
          <span className="menu-label">{t('reader-snooze')}</span>
          <span className="menu-key">B</span>
        </MenuItem>
        <MenuItem className="menu-item" onClick={() => onAction('archive')}>
          <Icon icon={Archive} size={14} />
          <span className="menu-label">{t('reader-archive')}</span>
          <span className="menu-key">E</span>
        </MenuItem>

        <MenuSeparator className="menu-sep" />

        {/* Destructive last and visually separated, so the mouse does not pass
            over "move to trash" on its way to something harmless. */}
        <MenuItem className="menu-item" onClick={() => onAction('spam')}>
          <Icon icon={ShieldAlert} size={14} />
          <span className="menu-label">{t('menu-spam')}</span>
          <span className="menu-key">!</span>
        </MenuItem>
        {/* "Move to trash" from inside the trash is a gesture with nowhere to
            go — it reads as broken. There, the same position and the same key
            mean the thing you actually wanted. */}
        <MenuItem
          className="menu-item danger"
          onClick={() => onAction(inTrash ? 'delete_forever' : 'trash')}
        >
          <Icon icon={Trash2} size={14} />
          <span className="menu-label">{inTrash ? t('delete-forever') : t('cmd-trash')}</span>
          <span className="menu-key">#</span>
        </MenuItem>
      </Menu>
    </MenuProvider>
  );
}
