/**
 * Whether selecting a row in this view means resuming a draft, not reading it.
 *
 * Drafts are unfinished mail. The composer fills the pane on the right; a
 * reading pane would show the same words in the one form they cannot edit.
 */
export function opensComposer(view: string): boolean {
  return view === 'drafts';
}
