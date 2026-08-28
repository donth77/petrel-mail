import { splitPath } from '../lib/folders';
import { Highlight } from './Highlight';

/**
 * A folder path drawn as faint context plus the name that identifies it.
 *
 * A picker list of full paths is mostly repetition — four rows opening with
 * `Archive/Yearly/` say that word four times and the thing telling them apart
 * is at the far end. Worse, the row ellipsises from the right, so the part it
 * throws away first is exactly that end: `Archive/Yearly/2023/Job Hunt 2023`
 * became `Archive/Yearly/2023/Job H…`, keeping three redundant words and
 * losing the only one that mattered.
 *
 * So the parent yields and the leaf does not. The parent shrinks first and
 * ellipsises to nothing if it has to; the leaf holds its width. Narrow, that
 * reads `Archive/Yea…/Job Hunt 2023`, which is the wrong way round from what
 * the row used to do and the right way round for reading it.
 *
 * The label itself is untouched — still the whole path — so fuzzy matching
 * still runs over `Archive` as well as `2023`. The hits simply land in two
 * spans instead of one, split at the boundary between them.
 */
export function PathLabel({ path, hits }: { path: string; hits: number[] }) {
  const { parent, leaf } = splitPath(path);
  if (!parent) return <span className="clip">{<Highlight text={leaf} hits={hits} />}</span>;
  return (
    <span className="path">
      <span className="path-lead">
        <Highlight text={parent} hits={hits.filter((i) => i < parent.length)} />
      </span>
      <span className="path-leaf">
        <Highlight
          text={leaf}
          hits={hits.filter((i) => i >= parent.length).map((i) => i - parent.length)}
        />
      </span>
    </span>
  );
}
