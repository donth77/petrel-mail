import type { ReactElement, ReactNode } from 'react';
import { useEffect, useState } from 'react';
import { Hovercard, HovercardAnchor, HovercardHeading, HovercardProvider } from '@ariakit/react';

/**
 * The subtree a collapsed rail has no room to draw, opened beside the icon.
 *
 * Collapsed, the rail is one column of 16px icons with no indentation and no
 * chevrons, so Archive/Yearly/2023 and a top-level Receipts render as the same
 * row wearing the same glyph — depth stops existing. The rail answers that by
 * drawing roots only at that width and handing the descendants to this: hover
 * an icon that has children and the whole tree opens to the right, named,
 * indented, every row one click from where you are.
 *
 * The card always opens fully unfolded. It is a surface you pass through on
 * the way somewhere, and one that appeared with its contents folded away would
 * be a puzzle rather than a shortcut — the rail's own fold state stays in the
 * rail, where there is a chevron to undo it with.
 */
export function RailFlyout({
  label,
  suppressed,
  anchor,
  children,
}: {
  /** Titles the card and names it for a screen reader — the label the
   *  collapsed rail cannot print beside the icon. */
  label: string;
  /** Forces the card shut and holds it shut. */
  suppressed: boolean;
  /** The rail row the card hangs off. A DOM element, not a component: Ariakit
   *  puts its hover handlers and a ref on whatever this renders. */
  anchor: ReactElement;
  children: ReactNode;
}) {
  const [open, setOpen] = useState(false);
  // Suppression is applied twice on purpose. Gating `open` shuts the card in
  // the same paint the drag starts in — an effect alone would leave it up for
  // a frame, over the row the pointer just grabbed. Clearing the state as well
  // stops it springing back when suppression lifts and the pointer has long
  // since moved somewhere else.
  useEffect(() => {
    if (suppressed) setOpen(false);
  }, [suppressed]);
  return (
    <HovercardProvider
      open={open && !suppressed}
      setOpen={setOpen}
      placement="right-start"
      // Long enough that crossing the rail on the way somewhere else does not
      // open three cards behind you; short enough that stopping on an icon
      // feels like it answered rather than stalled. The hide side is the more
      // forgiving of the two because the pointer has to travel off the icon
      // and across a gap to reach the card, and a card that closes during that
      // journey is a card you cannot use.
      showTimeout={160}
      hideTimeout={140}
    >
      {/* Focus opens it too. Hover is the gesture this is built around, but a
          collapsed rail is still a column of buttons somebody tabs through,
          and a keyboard that can reach the icon but never the folders under it
          would make those folders unreachable at this width. */}
      <HovercardAnchor render={anchor} onFocus={() => setOpen(true)} />
      <Hovercard portal gutter={8} unmountOnHide className="rail-flyout" aria-label={label}>
        <HovercardHeading className="rail-flyout-head">{label}</HovercardHeading>
        <div className="rail-flyout-tree">{children}</div>
      </Hovercard>
    </HovercardProvider>
  );
}
