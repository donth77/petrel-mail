import { useEffect } from 'react';
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
  useEffect(() => {
    if (!message) return;
    // Ten seconds, not three: undo is the safety net that lets archiving be
    // fast, and a net you have to catch in three seconds is not one.
    const h = setTimeout(onDone, onUndo ? 10000 : 2600);
    return () => clearTimeout(h);
  }, [message, onUndo, onDone]);

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
