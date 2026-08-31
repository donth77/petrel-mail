import { MenuItem, MenuSeparator } from '@ariakit/react';
import {
  Archive, Clock, ExternalLink, FolderClosed, Forward as ForwardIcon, Inbox, Mail,
  MailOpen, Maximize2, Minimize2, Reply as ReplyIcon, ReplyAll, ShieldAlert, Star,
  Tag as TagIcon, Trash2,
} from 'lucide-react';
import type { ActionKind, Thread } from '../lib/api';
import { Icon } from './Icon';
import { key } from '../lib/keys';
import { t } from '../lib/strings';

export type ThreadMenuProps = {
  thread: Thread;
  /** Which view is open, so the destructive item can mean the right thing. */
  view: string;
  onAction: (kind: ActionKind) => void;
  onMove: () => void;
  /** Back to the inbox — the inverse of archive and of filing, reachable
   *  without a drag. Absent where the store cannot say which inbox. */
  onMoveInbox?: () => void;
  onTag: () => void;
  onSnooze: () => void;
  /** Absent where there is nowhere to pop out to. */
  onPopOut?: () => void;
  /** Reading pane has the window to itself. */
  full?: boolean;
  /** Absent on a list row: filling the reading pane is not something you do to
   *  a conversation, and offering it there would be answering a question
   *  nobody asked of that row. */
  onToggleFull?: () => void;
  /** Shown when the menu was opened on more than one selected conversation, so
   *  it is obvious the next click applies to all of them. */
  count?: number;
  /** Reply to the conversation. Absent where no composer can open, and hidden
   *  on a multiple selection: there is no sensible message to reply to when
   *  the menu is acting on twelve conversations at once. */
  onReply?: (all: boolean) => void;
  onForward?: () => void;
};

/**
 * Everything you can do to a conversation, as menu items.
 *
 * Shared by the ⋮ button in the reading pane and the right-click menu on a
 * list row, because the alternative is two lists that agree today and drift by
 * the next feature — and the one people would notice is the one where an
 * action is missing from the place they happened to look.
 *
 * Items only. The two callers differ in how the menu is anchored, which is the
 * whole of the difference between them.
 */
export function ThreadMenuItems({
  thread, view, onAction, onMove, onMoveInbox, onTag, onSnooze, onPopOut, onToggleFull, full,
  count, onReply, onForward,
}: ThreadMenuProps) {
  const inTrash = view === 'trash';
  const many = (count ?? 1) > 1;

  return (
    <>
      {/* Says what the next click will hit. Right-clicking inside a selection
          acts on all of it, which is right but invisible without this. */}
      {many && <div className="menu-note">{t('menu-applies-to', { count: count ?? 1 })}</div>}

      {/* Reply first, because it is the most common thing anyone does to a
          conversation and a menu that buries it is a menu people stop opening.
          Hidden on a multiple selection: replying to twelve conversations is
          not a thing, and offering it would be offering a wrong answer. */}
      {(onReply || onForward) && !many && (
        <>
          {onReply && (
            <MenuItem className="menu-item" onClick={() => onReply(false)}>
              <Icon icon={ReplyIcon} size={14} />
              <span className="menu-label">{t('reader-reply')}</span>
              <span className="menu-key">R</span>
            </MenuItem>
          )}
          {onReply && (
            <MenuItem className="menu-item" onClick={() => onReply(true)}>
              <Icon icon={ReplyAll} size={14} />
              <span className="menu-label">{t('reader-reply-all')}</span>
              <span className="menu-key">A</span>
            </MenuItem>
          )}
          {onForward && (
            <MenuItem className="menu-item" onClick={onForward}>
              <Icon icon={ForwardIcon} size={14} />
              <span className="menu-label">{t('reader-forward')}</span>
              <span className="menu-key">F</span>
            </MenuItem>
          )}
          <MenuSeparator className="menu-sep" />
        </>
      )}

      {/* The view group, first and separated: the only items here that change
          nothing about the mail. They live in the menu rather than the header
          because the header's job is the subject — a control standing in front
          of it costs more than the click it saves, and both of these have
          somewhere else to be reached from. */}
      {(onToggleFull || onPopOut) && !many && (
        <>
          {onPopOut && (
            <MenuItem className="menu-item" onClick={onPopOut}>
              <Icon icon={ExternalLink} size={14} />
              <span className="menu-label">{t('reader-popout')}</span>
              <span className="menu-key">O</span>
            </MenuItem>
          )}
          {onToggleFull && (
            <MenuItem className="menu-item" onClick={onToggleFull}>
              <Icon icon={full ? Minimize2 : Maximize2} size={14} />
              <span className="menu-label">
                {full ? t('reader-shrink') : t('reader-expand')}
              </span>
              <span className="menu-key">\</span>
            </MenuItem>
          )}
          <MenuSeparator className="menu-sep" />
        </>
      )}

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
        <span className="menu-label">
          {thread.unread ? t('reader-mark-read') : t('reader-mark-unread')}
        </span>
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
      {/* Not offered where it already happened, nor to mail you wrote —
          archiving is a station in the life of something that arrived. */}
      {!['archive', 'sent', 'drafts', 'outbox'].includes(view) && (
        <MenuItem className="menu-item" onClick={() => onAction('archive')}>
          <Icon icon={Archive} size={14} />
          <span className="menu-label">{t('reader-archive')}</span>
          <span className="menu-key">E</span>
        </MenuItem>
      )}

      {/* The way back. Everywhere except the inbox itself — and except the
          views for mail you wrote, which was never in the inbox to return
          to. Restoring from Trash and Spam is this same item doing its most
          important job. */}
      {onMoveInbox && !['inbox', 'sent', 'drafts', 'outbox'].includes(view) && (
        <MenuItem className="menu-item" onClick={onMoveInbox}>
          <Icon icon={Inbox} size={14} />
          <span className="menu-label">{t('menu-move-inbox')}</span>
          <span className="menu-key">I</span>
        </MenuItem>
      )}

      <MenuSeparator className="menu-sep" />

      {/* Destructive last and visually separated, so the mouse does not pass
          over "move to trash" on its way to something harmless. */}
      <MenuItem className="menu-item" onClick={() => onAction('spam')}>
        <Icon icon={ShieldAlert} size={14} />
        <span className="menu-label">{t('menu-spam')}</span>
        <span className="menu-key">!</span>
      </MenuItem>
      {/* "Move to trash" from inside the trash is a gesture with nowhere to go —
          it reads as broken. There, the same position and the same key mean the
          thing you actually wanted. */}
      <MenuItem
        className="menu-item danger"
        onClick={() => onAction(inTrash ? 'delete_forever' : 'trash')}
      >
        <Icon icon={Trash2} size={14} />
        <span className="menu-label">{inTrash ? t('delete-forever') : t('cmd-trash')}</span>
        <span className="menu-key">#</span>
      </MenuItem>
    </>
  );
}
