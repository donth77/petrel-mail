/** Close-on-click-away, spelled out rather than inherited.
 *
 * Every dialog here is laid out the same way: a full-width wrapper that centres
 * a panel. The wrapper *is* the Ariakit dialog element, so a click on the empty
 * space beside the panel lands inside the dialog as far as the library is
 * concerned, and its own outside-interaction handling never fires. The result
 * is a dialog you cannot dismiss by clicking away from it, which is the one
 * gesture everybody tries first.
 *
 * `target === currentTarget` is exactly "the wrapper itself, not anything in
 * it", so this closes on the empty space and never on a click that happened to
 * bubble up from a control inside the panel.
 */
export function clickAway(onClose: () => void) {
  return {
    onClick: (e: React.MouseEvent) => {
      if (e.target === e.currentTarget) onClose();
    },
  };
}
