import { Menu, MenuButton, MenuItem, MenuProvider, MenuSeparator } from '@ariakit/react';
import { Check, ChevronDown, Settings2 } from 'lucide-react';
import type { Account } from '../lib/api';
import { Icon } from './Icon';
import { t } from '../lib/strings';

type Props = {
  accounts: Account[];
  /** The address actually signed in, which may be known before the account row
   *  is, so it is passed separately rather than inferred from the list. */
  current: string;
  unread: number;
  accountColor: string;
  onSwitch: (index: number) => void;
  onSettings: () => void;
};

/**
 * The account header, which is a menu rather than a label with a chevron
 * painted on it.
 *
 * It had the chevron and no behaviour — a control that looks like it opens
 * something and does nothing is worse than a plain label, because it invites
 * the click and then teaches you the app is broken. One account still gets a
 * menu: it is where "add another" and "account settings" live, and both are
 * reachable in one gesture from the thing they are about.
 */
export function AccountMenu({
  accounts,
  current,
  unread,
  accountColor,
  onSwitch,
  onSettings,
}: Props) {
  return (
    <MenuProvider placement="bottom-start">
      <MenuButton className="account">
        {/* The account's own colour, not the app accent — the whole point of
            setting one is telling accounts apart at a glance. */}
        <span className="dot" style={{ background: accountColor }} />
        {/* A class, not an inline style: inline wins over every stylesheet rule,
            so the collapsed state could not shrink this and the leftover width
            pushed the dot off centre. */}
        <span className="account-text">
          <span className="clip" style={{ display: 'block', fontSize: 12.5, fontWeight: 600 }}>
            {current}
          </span>
          <span className="mono" style={{ fontSize: 10, color: 'var(--ink3)' }}>
            {t('list-unread', { count: unread })}
          </span>
        </span>
        <Icon icon={ChevronDown} size={13} />
      </MenuButton>

      {/* Portalled, or the menu renders inside the rail's flex column: it
          becomes a flex child, widens the rail's scroll box and shifts every
          item left by a few pixels the moment it opens. */}
      <Menu
        portal
        gutter={4}
        className="menu"
        aria-label={t('rail-switch-account')}
      >
        {accounts.map((a, i) => (
          <MenuItem key={a.id} className="menu-item" onClick={() => onSwitch(i + 1)}>
            <span
              className="picker-dot"
              aria-hidden="true"
              style={{ background: a.color || 'var(--ink3)' }}
            />
            <span className="menu-label">{a.email}</span>
            {a.email === current && <Icon icon={Check} size={13} />}
          </MenuItem>
        ))}
        {accounts.length > 0 && <MenuSeparator className="menu-sep" />}

        <MenuItem className="menu-item" onClick={onSettings}>
          <Icon icon={Settings2} size={14} />
          <span className="menu-label">{t('settings-accounts')}</span>
        </MenuItem>
      </Menu>
    </MenuProvider>
  );
}
