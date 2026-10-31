import { useEffect, useRef } from 'react';
import { Menu, MenuItem, MenuProvider, useMenuStore } from '@ariakit/react';
import { AtSign, Copy } from 'lucide-react';
import { t } from '../lib/strings';
import {
  dismissClicks,
  focusDismissesMenu,
  pressDismissesMenu,
  writeClipboard,
} from '../lib/recipient-menu';
import { Icon } from './Icon';

type Props = {
  at: { x: number; y: number };
  copyText: string;
  addressText: string;
  onClose: () => void;
};

/**
 * Copy and Copy email address on a recipient name in the reader header.
 *
 * Same anchoring contract as RowMenu: portal, fitViewport, a point rect at the
 * click — so a name at the bottom of the window still opens upward.
 *
 * A system menu leaves when you press anywhere else, including the message,
 * which is another document and never delivers that click to this window.
 * Escape leaves too. Both are listened for here. A dismissing press on the
 * collapse button is eaten with its click, because the name sits inside that
 * button and the press would otherwise close the message as well. Anywhere
 * else the press goes on to what it landed on.
 */
export function RecipientMenu({ at, copyText, addressText, onClose }: Props) {
  const menu = useMenuStore({
    open: true,
    setOpen: (open) => {
      if (!open) onClose();
    },
  });
  const close = useRef(onClose);
  close.current = onClose;

  useEffect(() => {
    menu.setAnchorElement(null);
  }, [menu, at.x, at.y]);

  useEffect(() => {
    const clicks = dismissClicks();
    // Auto-focus moves into the menu on the next microtask. A blur in that
    // same turn is that move, not the reader leaving the menu.
    let armed = false;
    const arm = requestAnimationFrame(() => {
      armed = true;
    });
    const dismiss = () => close.current();
    const targetOf = (target: EventTarget | null) => {
      if (target instanceof Element) return target;
      if (target instanceof Node) return target.parentElement;
      return null;
    };
    const onPointerDown = (event: PointerEvent) => {
      if (!pressDismissesMenu(targetOf(event.target))) return;
      // A right-click still has to reach the name under it, so that menu can
      // move. A left press on the collapse button must not close the message
      // too. The click arrives after this menu has unmounted, so the window's
      // rule holds it, and lets it go with the next press or key.
      clicks.dismissedBy(event);
      dismiss();
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.preventDefault();
      event.stopPropagation();
      dismiss();
    };
    const onFocusIn = (event: FocusEvent) => {
      const target = event.target instanceof Element ? event.target : null;
      if (focusDismissesMenu(target)) dismiss();
    };
    const onBlur = () => {
      if (armed) dismiss();
    };
    window.addEventListener('pointerdown', onPointerDown, true);
    window.addEventListener('keydown', onKeyDown, true);
    window.addEventListener('focusin', onFocusIn, true);
    window.addEventListener('blur', onBlur);
    return () => {
      cancelAnimationFrame(arm);
      window.removeEventListener('pointerdown', onPointerDown, true);
      window.removeEventListener('keydown', onKeyDown, true);
      window.removeEventListener('focusin', onFocusIn, true);
      window.removeEventListener('blur', onBlur);
    };
  }, []);

  return (
    <MenuProvider store={menu}>
      <Menu
        portal
        fitViewport
        className="menu"
        data-recipient-menu=""
        aria-label={t('recipient-menu')}
        getAnchorRect={() => ({ x: at.x, y: at.y, width: 0, height: 0 })}
        onClose={onClose}
      >
        <MenuItem
          className="menu-item"
          onClick={() => {
            void writeClipboard(copyText).finally(onClose);
          }}
        >
          <Icon icon={Copy} size={14} />
          <span className="menu-label">{t('menubar-copy')}</span>
        </MenuItem>
        <MenuItem
          className="menu-item"
          onClick={() => {
            void writeClipboard(addressText).finally(onClose);
          }}
        >
          <Icon icon={AtSign} size={14} />
          <span className="menu-label">{t('recipient-copy-address')}</span>
        </MenuItem>
      </Menu>
    </MenuProvider>
  );
}
