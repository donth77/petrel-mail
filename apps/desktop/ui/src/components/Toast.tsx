import { useEffect, useRef } from 'react';
import { t } from '../lib/strings';

/**
 * Transient status. Uses a polite live region so a screen reader hears it
 * without having focus yanked — the same channel the undo toast will use, so
 * announcements stay in one place rather than accumulating per feature.
 */
export function Toast({
  message,
  onUndo,
  onDone,
}: {
  message: string | null;
  onUndo?: () => void;
  onDone: () => void;
}) {
  // Held in a ref, and deliberately not in the dependency list below.
  //
  // Both callbacks arrive as inline arrows, so they are new objects on every
  // render. Depending on them re-ran this effect each time, which cleared the
  // timer and started it again — and the app re-renders on a status poll every
  // five seconds, or every 400ms while a sync is running. A toast raised during
  // a sync therefore never expired at all: its dismissal was pushed back before
  // it could ever arrive.
  const done = useRef(onDone);
  done.current = onDone;

  // Whether undo is offered, rather than the function that does it: this is the
  // part that changes the duration, and it is a boolean that stays stable.
  const undoable = onUndo != null;

  useEffect(() => {
    if (!message) return;
    // Ten seconds, not three: undo is the safety net that lets archiving be
    // fast, and a net you have to catch in three seconds is not one.
    const h = setTimeout(() => done.current(), undoable ? 10000 : 2600);
    return () => clearTimeout(h);
  }, [message, undoable]);

  return (
    <div className="toast-region" role="status" aria-live="polite">
      {message && (
        <div className="toast">
          <span>{message}</span>
          {onUndo && (
            <button type="button" className="toast-undo" onClick={onUndo}>
              {t('undo')} <span className="kbd">Z</span>
            </button>
          )}
        </div>
      )}
    </div>
  );
}
