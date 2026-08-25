import { Dialog, DialogDismiss } from '@ariakit/react';
import { X } from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import { Icon } from './Icon';
import { t } from '../lib/strings';

/**
 * A one-field dialog for naming something new.
 *
 * The rail's inline inputs remain the expanded path — naming in place, where
 * the thing will appear, is the better gesture when there is room for it.
 * This exists for the collapsed rail, where there is no row to type into and
 * pressing + should not force the rail open just to ask for a name.
 */
export function NameDialog({
  open,
  title,
  placeholder,
  icon,
  onClose,
  onSubmit,
}: {
  open: boolean;
  title: string;
  placeholder: string;
  icon: LucideIcon;
  onClose: () => void;
  onSubmit: (name: string) => void;
}) {
  return (
    <Dialog
      open={open}
      onClose={onClose}
      backdrop={<div className="palette-scrim" onClick={onClose} />}
      className="picker name-dialog"
      aria-label={title}
    >
      <div className="picker-head">
        <Icon icon={icon} size={14} />
        <input
          className="picker-input"
          autoFocus
          autoComplete="off"
          placeholder={placeholder}
          aria-label={title}
          onKeyDown={(e) => {
            // Stopped so the app's single-key shortcuts stay quiet while a
            // name is being typed — the same rule the inline inputs follow.
            e.stopPropagation();
            if (e.key === 'Escape') {
              onClose();
              return;
            }
            if (e.key !== 'Enter') return;
            const name = e.currentTarget.value.trim();
            onClose();
            if (name) onSubmit(name);
          }}
        />
        <DialogDismiss className="close-btn" aria-label={t('close')}>
          <Icon icon={X} size={15} />
        </DialogDismiss>
      </div>
      <div className="picker-foot">{title}</div>
    </Dialog>
  );
}
