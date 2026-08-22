import { useEffect } from 'react';
import { Menu, MenuProvider, useMenuStore } from '@ariakit/react';
import { t } from '../lib/strings';
import { ThreadMenuItems, type ThreadMenuProps } from './ThreadMenuItems';

type Props = ThreadMenuProps & {
  /** Where the pointer was, in viewport coordinates. */
  at: { x: number; y: number };
  onClose: () => void;
};

/**
 * The right-click menu on a conversation row.
 *
 * Same items as the ⋮ button, anchored to the pointer instead of to a button —
 * which is the whole difference, and why the items live in one shared place.
 *
 * Anchored with a zero-size rect at the click point, the standard way to give a
 * popover a position rather than an element: the menu then flips and shifts on
 * its own near an edge, so a right-click at the bottom of the list opens
 * upward instead of off-screen.
 */
export function RowMenu({ at, onClose, ...items }: Props) {
  const menu = useMenuStore({
    open: true,
    setOpen: (open) => {
      if (!open) onClose();
    },
  });

  // The anchor is a point, and the point moves with each right-click — so it is
  // re-rendered rather than set once, or the second menu would open where the
  // first one did.
  useEffect(() => {
    menu.setAnchorElement(null);
  }, [menu, at.x, at.y]);

  return (
    <MenuProvider store={menu}>
      <Menu
        portal
        className="menu"
        aria-label={t('reader-more')}
        getAnchorRect={() => ({ x: at.x, y: at.y, width: 0, height: 0 })}
        // Closing on a click elsewhere and on Escape is the whole contract of a
        // context menu; Ariakit does both, and this makes sure the state above
        // hears about it rather than leaving a dead menu mounted.
        onClose={onClose}
      >
        <ThreadMenuItems {...items} />
      </Menu>
    </MenuProvider>
  );
}
