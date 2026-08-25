import type { Folder } from './api';

/**
 * Where a folder lands when it is dragged to Archive or Trash.
 *
 * Ordinarily that is the role folder's own path — Namecheap's `Archive` takes
 * children happily, and `Archive/2026` reads as exactly what it is. Gmail is
 * the exception: its archive wears the reserved name `[Gmail]/All Mail`, and
 * while the server will technically accept a rename into that namespace, the
 * result is a junk label other clients render raw — which is how a dragged
 * folder once surfaced as a literal `[Gmail]` tree in the rail. For those
 * accounts the anchor is an ordinary label beside the system ones: `Archive`
 * or `Trash`, which Gmail accepts, auto-creating the parent.
 *
 * Undefined when the account has no folder wearing the role at all — with no
 * anchor there is nowhere to nest, and the drop is not offered.
 */
export function nestableRolePath(
  folders: Folder[],
  role: 'archive' | 'trash',
): string | undefined {
  const fallback = role === 'archive' ? 'Archive' : 'Trash';
  const path = folders.find((f) => f.role === role)?.path;
  // No folder wears the role at all — Namecheap marks no \Archive — but a
  // plain top-level folder with the role's own name is the same place by
  // convention, and it is where this app has been filing.
  if (!path) {
    return folders.some((f) => !f.role && f.path === fallback) ? fallback : undefined;
  }
  if (path.startsWith('[Gmail]')) return fallback;
  return path;
}

/** Whether a path sits at or under an anchor, on either separator. */
export function underAnchor(path: string, anchor: string | undefined): boolean {
  return (
    anchor !== undefined &&
    (path === anchor ||
      (path.startsWith(anchor) && (path[anchor.length] === '/' || path[anchor.length] === '.')))
  );
}
