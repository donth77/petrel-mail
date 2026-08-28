import { useMemo, useState } from 'react';
import { underAnyClosed } from './folders';

/** What a foldable row has to say about itself. */
type Foldable = { label: string; hasChildren?: boolean; anchor?: 'archive' | 'trash' };

/**
 * Folding a folder tree that is drawn as a flat list.
 *
 * Shared by the two pickers so they cannot disagree about what is folded — the
 * same reason the tree itself was lifted out of the rail. It works on paths
 * rather than on tree nodes, because by the time a picker has a list it has
 * already been flattened: a row is hidden when any folded row is one of its
 * ancestors, which a path can answer on its own.
 *
 * Archive and Trash arrive folded, exactly as they do in the rail: what hangs
 * off them is mail already dealt with, and on a real mailbox they are most of
 * the list. Everything else arrives open. A row absent from `folded` takes that
 * default, so the anchors can be unfolded by hand and stay unfolded, and a
 * folder list that loads a moment later does not un-fold what you just opened.
 */
export function useFolderFold(options: readonly Foldable[]) {
  const [folded, setFolded] = useState<Record<string, boolean>>({});

  const anchors = useMemo(
    () => new Set(options.filter((o) => o.anchor).map((o) => o.label)),
    [options],
  );
  const foldedByDefault = (path: string) => anchors.has(path);
  const isOpen = (path: string) => !(folded[path] ?? foldedByDefault(path));

  const closed = useMemo(() => {
    const set = new Set<string>();
    for (const o of options) {
      if (o.hasChildren && (folded[o.label] ?? anchors.has(o.label))) set.add(o.label);
    }
    return set;
  }, [options, folded, anchors]);

  return {
    isOpen,
    /** Whether a row is folded away inside something above it. */
    hidden: (path: string) => underAnyClosed(path, closed),
    toggle: (path: string) =>
      setFolded((prev) => ({ ...prev, [path]: !(prev[path] ?? foldedByDefault(path)) })),
  };
}
