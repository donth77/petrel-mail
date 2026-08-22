import { useEffect, useState } from 'react';
import { Dialog, DialogDismiss } from '@ariakit/react';
import {
  Bell, Database, Mail, PencilLine, Shield, SunMoon, User, X,
  type LucideIcon,
} from 'lucide-react';
import { Accounts } from './settings/Accounts';
import { Appearance } from './settings/Appearance';
import { Composing } from './settings/Composing';
import { Notifications } from './settings/Notifications';
import { Identities } from './settings/Identities';
import { Privacy } from './settings/Privacy';
import { Storage } from './settings/Storage';
import { Icon } from './Icon';
import { Tip } from './Tip';
import { t, type StringId } from '../lib/strings';

type PaneId =
  | 'accounts' | 'identities' | 'composing' | 'notifications'
  | 'appearance' | 'privacy' | 'storage';

const PANES: { id: PaneId; label: StringId; icon: LucideIcon }[] = [
  { id: 'accounts', label: 'settings-accounts', icon: Mail },
  { id: 'identities', label: 'settings-identities', icon: User },
  { id: 'composing', label: 'settings-composing', icon: PencilLine },
  { id: 'notifications', label: 'settings-notifications', icon: Bell },
  { id: 'appearance', label: 'settings-appearance', icon: SunMoon },
  { id: 'privacy', label: 'settings-privacy', icon: Shield },
  { id: 'storage', label: 'settings-storage', icon: Database },
];

// Help is deliberately not in this list. Settings is where things are
// configured, and the shortcut map has nothing to configure — it was a nav item
// that closed the dialog and opened an overlay instead of showing a pane, which
// is a row that lies about what it is. The rail has its own Help. If shortcuts
// ever become remappable, that pane belongs here; the reference does not.

type Props = {
  open: boolean;
  /** Which pane to land on. Opening Settings from "Accounts" and arriving at
   *  Appearance is the kind of small betrayal that teaches people to distrust
   *  every other shortcut in the app. */
  pane?: PaneId;
  onClose: () => void;
  /** A plain status line. Not the "not built" channel — routing a
   *  successful export through that reported it as a missing feature. */
  onMessage: (text: string) => void;
};

export function Settings({ open, pane: requested, onClose, onMessage }: Props) {
  const [pane, setPane] = useState<PaneId>(requested ?? 'appearance');

  // Follow the request each time the dialog opens, not once on mount: the
  // component stays mounted between openings, so a value read at mount would
  // be whatever the first caller asked for, forever.
  useEffect(() => {
    if (open && requested) setPane(requested);
  }, [open, requested]);

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
              onClick={() => setPane(p.id)}
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
          {pane === 'accounts' && <Accounts />}
          {pane === 'notifications' && <Notifications />}
          {pane === 'composing' && <Composing />}
          {pane === 'storage' && <Storage onMessage={onMessage} />}
          {pane === 'privacy' && <Privacy />}
          {pane === 'identities' && <Identities onMessage={onMessage} />}
          {pane !== 'appearance' && pane !== 'accounts' && pane !== 'notifications' && pane !== 'composing' && pane !== 'storage' && pane !== 'privacy' && pane !== 'identities' && (
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
