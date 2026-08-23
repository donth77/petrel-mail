import type { Dragging } from '../lib/useDrag';
import { t } from '../lib/strings';

/**
 * What the pointer is carrying, drawn under it.
 *
 * The browser's own drag image was a photograph of the row, which said nothing
 * useful when several rows were moving and could not be styled at all. This is
 * an element, so it can say how many and follow the pointer exactly.
 *
 * Transparent to the pointer, or it would be the thing under the cursor and the
 * drag would hit-test itself instead of the destination beneath it.
 */
export function DragPreview({ drag }: { drag: Dragging | null }) {
  if (!drag) return null;
  return (
    <div
      className="drag-preview"
      data-over={drag.over || drag.overRow !== null ? true : undefined}
      style={{ transform: `translate3d(${drag.x + 12}px, ${drag.y + 10}px, 0)` }}
      aria-hidden="true"
    >
      {drag.payload.kind === 'threads' && drag.payload.ids.length > 1
        ? t('drag-count', { count: String(drag.payload.ids.length) })
        : drag.label}
    </div>
  );
}
