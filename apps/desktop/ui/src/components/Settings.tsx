import { useState } from 'react';
import { Dialog, DialogDismiss } from '@ariakit/react';
import {
  Archive, Bell, CircleHelp, Database, Mail, PencilLine, Shield, SunMoon, User, X,
  type LucideIcon,
} from 'lucide-react';
import { Accounts } from './settings/Accounts';
import { Appearance } from './settings/Appearance';
import { Notifications } from './settings/Notifications';
import { Icon } from './Icon';
import { Tip } from './Tip';
import { t, type StringId } from '../lib/strings';

type PaneId =
  | 'accounts' | 'identities' | 'composing' | 'notifications'
  | 'appearance' | 'privacy' | 'storage' | 'help';

const PANES: { id: PaneId; label: StringId; icon: LucideIcon }[] = [
  { id: 'accounts', label: 'settings-accounts', icon: Mail },
  { id: 'identities', label: 'settings-identities', icon: User },
  { id: 'composing', label: 'settings-composing', icon: PencilLine },
  { id: 'notifications', label: 'settings-notifications', icon: Bell },
  { id: 'appearance', label: 'settings-appearance', icon: SunMoon },
  { id: 'privacy', label: 'settings-privacy', icon: Shield },
  { id: 'storage', label: 'settings-storage', icon: Database },
  { id: 'help', label: 'rail-help', icon: CircleHelp },
];

type Props = {
  open: boolean;
  onClose: () => void;
  onOpenHelp: () => void;
  onNotImplemented: (label: string) => void;
};

export function Settings({ open, onClose, onOpenHelp, onNotImplemented }: Props) {
  const [pane, setPane] = useState<PaneId>('appearance');

  return (
    <Dialog
      open={open}
      onClose={onClose}
      className="settings-backdrop"
      backdrop={<div className="palette-scrim" />}
      aria-label={t('settings-title')}
    >
      <div className="settings">
        <nav className="settings-nav" aria-label={t('settings-title')}>
          <div className="settings-title">{t('settings-title')}</div>
          {PANES.map((p) => (
            <button
              key={p.id}
              type="button"
              className="navitem"
              aria-current={pane === p.id ? 'page' : undefined}
              onClick={() => {
                // Help is the overlay we already have, not a pane that
                // duplicates it — one keyboard map, one place to fix it.
                if (p.id === 'help') {
                  onClose();
                  onOpenHelp();
                } else setPane(p.id);
              }}
            >
              <Icon icon={p.icon} />
              {t(p.label)}
            </button>
          ))}
        </nav>

        <div className="settings-pane">
          <Tip label={t('close-title')} placement="bottom">
            <DialogDismiss className="close-btn settings-esc" aria-label={t('close')}>
              <Icon icon={X} size={15} />
            </DialogDismiss>
          </Tip>
          {pane === 'appearance' && <Appearance />}
          {pane === 'accounts' && <Accounts onNotImplemented={onNotImplemented} />}
          {pane === 'notifications' && <Notifications />}
          {pane !== 'appearance' && pane !== 'accounts' && pane !== 'notifications' && (
            <div className="empty">
              <h2>{t(PANES.find((p) => p.id === pane)!.label)}</h2>
              <p>{t('settings-not-built')}</p>
            </div>
          )}
        </div>
      </div>
    </Dialog>
  );
}
