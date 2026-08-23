import { useCallback, useEffect, useRef, useState } from 'react';

/**
 * Stops a file dropped on the window from replacing the application.
 *
 * A webview's default answer to a dropped file is to navigate to it. Drop a
 * photo anywhere that is not listening and the window stops being Petrel and
 * becomes that photo, full screen, with no way back — the app looks frozen
 * because it is no longer running in that window.
 *
 * So every drop is cancelled at the window, and the places that actually want
 * one handle it on the way past. This has to be registered in every window the
 * app opens, not only the one with a composer in it: the reading window and the
 * compose window navigate away just as readily.
 */
/**
 * Whether this is text being dragged into somewhere text belongs.
 *
 * The one case where the engine's own handling is wanted, so it is the one case
 * allowed through — and the test is written to fail closed. A drag has to
 * positively declare text *and* be over a field to be let past; anything the
 * engine describes oddly, or does not describe at all, is cancelled.
 *
 * That direction matters more than it looks. Failing open here means a file
 * dropped on the message body navigates the window to that file, and the
 * application is simply gone — which is precisely what a rich-text body would
 * have caused, being editable and the most natural place to drop a picture.
 */
function droppingTextIntoAField(e: DragEvent): boolean {
  const types = e.dataTransfer?.types;
  if (!types || types.length === 0) return false;
  const carriesFiles = Array.from(types).includes('Files');
  const carriesText = Array.from(types).some((t) => t.startsWith('text/'));
  if (carriesFiles || !carriesText) return false;
  const el = e.target as HTMLElement | null;
  // The message body is deliberately not in this list: it is editable, but a
  // picture dropped on it should become an attachment, not a navigation.
  return !!el?.closest?.('input, textarea');
}

export function useDropGuard() {
  useEffect(() => {
    // Not conditional on `dataTransfer.types` naming "Files". That list is the
    // engine's to describe and it does not describe it identically everywhere,
    // and a guard that consults it fails open — the one outcome that must not
    // happen here, because failing open means the window navigates away and
    // the application is gone.
    //
    // Dropping is cancelled outright: nothing in this app wants the browser's
    // default, which is to leave and display the file.
    const stopDrop = (e: DragEvent) => e.preventDefault();

    // Dragging over is cancelled too — without it the engine never offers a
    // drop at all — with one exception: dragged *text* over somewhere text
    // belongs. A subject line should still accept a dragged word.
    const stopDragOver = (e: DragEvent) => {
      if (droppingTextIntoAField(e)) return;
      e.preventDefault();
    };

    window.addEventListener('dragover', stopDragOver);
    window.addEventListener('drop', stopDrop);
    return () => {
      window.removeEventListener('dragover', stopDragOver);
      window.removeEventListener('drop', stopDrop);
    };
  }, []);
}

/**
 * A region that accepts files dragged in from the desktop.
 *
 * Whether a drag is overhead is decided by a timer rather than by counting
 * `dragenter` against `dragleave`. Those two fire on every child the pointer
 * crosses and are not reliably balanced — a count that drifts by one leaves the
 * region stuck highlighted, and a boolean toggled by both flickers on and off
 * as the pointer moves across the fields. `dragover` repeats while a drag is
 * overhead, so "recently heard one" is the honest question, and the flicker
 * cannot happen because nothing turns it off except silence.
 */
export function useFileDropZone(onFiles: (files: FileList) => void) {
  const [over, setOver] = useState(false);
  const timer = useRef<number | null>(null);

  const stopClock = () => {
    if (timer.current !== null) window.clearTimeout(timer.current);
    timer.current = null;
  };

  useEffect(() => stopClock, []);

  const onDragOver = useCallback((e: React.DragEvent) => {
    // Cancelled whatever is being dragged, so a drop is always possible here;
    // the drop itself decides whether there were files. Only the sign shown to
    // the reader depends on the engine's description of the drag, so the worst
    // an unfamiliar one costs is a missing highlight rather than a lost file.
    if (droppingTextIntoAField(e.nativeEvent)) return;
    e.preventDefault();
    if (!e.dataTransfer.types.includes('Files')) return;
    e.dataTransfer.dropEffect = 'copy';
    setOver(true);
    stopClock();
    // Comfortably longer than the interval between `dragover` events, so an
    // ordinary pause in a slow drag does not read as the drag having left.
    timer.current = window.setTimeout(() => setOver(false), 160);
  }, []);

  const onDrop = useCallback(
    (e: React.DragEvent) => {
      e.preventDefault();
      stopClock();
      setOver(false);
      // `files` at drop time is the definitive answer, and the only one worth
      // acting on: it is the actual list, not the engine's description of what
      // the drag was carrying.
      if (e.dataTransfer.files.length > 0) onFiles(e.dataTransfer.files);
    },
    [onFiles],
  );

  return { over, dropProps: { onDragOver, onDrop } };
}
