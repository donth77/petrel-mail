import { useEffect } from 'react';

/**
 * Transient status. Uses a polite live region so a screen reader hears it
 * without having focus yanked — the same channel the undo toast will use, so
 * announcements stay in one place rather than accumulating per feature.
 */
export function Toast({ message, onDone }: { message: string | null; onDone: () => void }) {
  useEffect(() => {
    if (!message) return;
    const h = setTimeout(onDone, 2600);
    return () => clearTimeout(h);
  }, [message, onDone]);

  return (
    <div className="toast-region" role="status" aria-live="polite">
      {message && <div className="toast">{message}</div>}
    </div>
  );
}
