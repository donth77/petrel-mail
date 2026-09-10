import type { ActionKind } from './api';

/** The views that are about where a conversation is filed. Filing it
 *  somewhere else takes it out of these and only these. */
function isPlacementView(view: string): boolean {
  return (
    view === 'inbox' ||
    view === 'archive' ||
    view === 'sent' ||
    view === 'drafts' ||
    view === 'spam' ||
    view === 'trash' ||
    view.startsWith('folder:')
  );
}

/** Whether an action takes a conversation out of the list you are looking at.
 *
 *  This cannot be read off the action alone. Archiving removes a row from the
 *  inbox and from trash, but not from the archive; unstarring removes it from
 *  Starred and from nowhere else. Deciding by action alone left rows sitting in
 *  lists they no longer belonged to. */
export function leavesView(kind: ActionKind, view: string): boolean {
  // Gone entirely, so not here, wherever here is.
  if (kind === 'delete_forever') return true;

  // Filed somewhere specific. That changes where the conversation sits, and
  // only the views that are about where it sits lose it. Starred, Snoozed
  // and a tag are about a mark on the conversation, which filing does not
  // touch: this used to say "not here" for every view, and a starred
  // conversation dragged onto a folder from Starred vanished from the list
  // as if the star had gone with it. The store had kept it all along.
  if (kind === 'move') return isPlacementView(view);

  // Trash and spam are exclusive placements on both kinds of provider — the
  // conversation leaves wherever it was. So it leaves whatever list you happen
  // to be looking at, unless that list is where it lands.
  //
  // This used to be enumerated view by view, and the enumeration was wrong:
  // Sent, Drafts, Snoozed and every tag view were all listed as places nothing
  // moves out of, so binning something from any of them left the row sitting
  // there until a refresh took it away.
  if (kind === 'trash') return view !== 'trash';
  if (kind === 'spam') return view !== 'spam';

  // Out of the inbox, and out of a bin it is being rescued from. Stars and tags
  // survive archiving, so those views keep the conversation.
  if (kind === 'archive') return view === 'inbox' || view === 'trash' || view === 'spam';

  if (kind === 'snooze') return view === 'inbox';
  if (kind === 'unsnooze') return view === 'snoozed';
  if (kind === 'unstar') return view === 'starred';

  // Untagging is deliberately not here. The row only leaves if the tag removed
  // is the one being viewed, and this cannot see which tag was passed — so it
  // leaves the row alone rather than risk removing one the user is still
  // looking at. The next load has it right.
  return false;
}
