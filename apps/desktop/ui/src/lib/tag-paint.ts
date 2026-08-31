import type { Thread } from './api';

/**
 * Repainting a tag everywhere it is shown.
 *
 * The rail holds the tag list, and that is the authority on a tag's colour. A
 * row does not read it: it carries its own copy of every tag on it, taken when
 * the row was built. So recolouring a tag and repainting only the rail left
 * every chip in the conversation list on the old colour, and the two disagreed
 * until something reloaded the rows.
 *
 * Matched by id rather than by name, because a tag's name is the thing most
 * likely to be changing at the same time.
 */
export function repaintTag<T extends Pick<Thread, 'tags'>>(
  rows: T[],
  tagId: number,
  colour: string,
): T[] {
  let touched = false;
  const next = rows.map((row) => {
    if (!row.tags.some((x) => x.id === tagId)) return row;
    touched = true;
    return {
      ...row,
      tags: row.tags.map((x) => (x.id === tagId ? { ...x, colour } : x)),
    };
  });
  // The same array back when nothing carried the tag, so a list that did not
  // change does not re-render on a recolour that had nothing to do with it.
  return touched ? next : rows;
}
