import type { Draft } from '../components/Compose';
import type { Finished } from './useDroppedAttachments';

/**
 * A save that wrote the message without a file the person saw on it.
 *
 * Thrown to Send, Send later, Pop out and a popped-out window's Close, once
 * the row is written: the words are kept, and the action stops rather than
 * take the message away without the file. The copy has already said why, so
 * there is nothing here for the person to read. It carries the message as
 * written, with its row, for a composer to take back. No file names: an
 * error can end up in a log.
 */
export class FilesLeftOut extends Error {
  readonly draft: Draft;
  constructor(id: number, draft: Draft) {
    super('files left out');
    this.name = 'FilesLeftOut';
    this.draft = { ...draft, savedId: id };
  }
}

/**
 * The row a save wrote, or `FilesLeftOut` when the save was for an action
 * that takes the message out of the composer (`whole`) and a dropped file
 * could not come with it. An autosave is not `whole`: it keeps what there
 * is, and the person carries on writing.
 */
export function keptWhole(id: number, finished: Finished, whole: boolean): number {
  if (whole && finished.leftOut.length > 0) throw new FilesLeftOut(id, finished.draft);
  return id;
}
