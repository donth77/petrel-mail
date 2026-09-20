import { useEffect, useRef, useState } from 'react';
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
  suggested,
  confirmLabel,
  onClose,
  onSubmit,
}: {
  open: boolean;
  title: string;
  placeholder: string;
  icon: LucideIcon;
  /** A name offered rather than imposed: selected, so typing replaces it and
   *  Enter accepts it. Saving a search prefills from the query. */
  suggested?: string;
  /** The verb on the button. Enter has always worked; a dialog with no visible
   *  way to say yes looks like a dialog that cannot be finished. */
  confirmLabel: string;
  onClose: () => void;
  onSubmit: (name: string) => void;
}) {
  const field = useRef<HTMLInputElement>(null);
  // Held so the button knows whether there is anything to save. Enter reads the
  // field directly, as it always did.
  //
  // Synced rather than only initialised: this dialog stays mounted while it is
  // closed, so the initial value was whatever the suggestion was when the
  // window started — empty — and the Save button sat disabled over a field with
  // a name already in it.
  const [name, setName] = useState(suggested ?? '');
  useEffect(() => {
    if (open) setName(suggested ?? '');
  }, [open, suggested]);
  const commit = () => {
    const next = (field.current?.value ?? name).trim();
    onClose();
    if (next) onSubmit(next);
  };

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
          // Keyed on the suggestion so a second opening starts from the new
          // one rather than from whatever the last one left behind.
          key={suggested ?? ''}
          ref={field}
          className="picker-input"
          autoFocus
          autoComplete="off"
          defaultValue={suggested ?? ''}
          onFocus={(e) => e.currentTarget.select()}
          onChange={(e) => setName(e.currentTarget.value)}
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
            commit();
          }}
        />
        <DialogDismiss className="close-btn" aria-label={t('close')}>
          <Icon icon={X} size={15} />
        </DialogDismiss>
      </div>
      {/* The verb, and a way out that is not the X. Until this row existed the
          only way to finish was Return, which the dialog never said. */}
      <div className="name-dialog-foot">
        <DialogDismiss className="reply">{t('cancel')}</DialogDismiss>
        <button type="button" className="reply primary" onClick={commit} disabled={!name.trim()}>
          {confirmLabel}
        </button>
      </div>
    </Dialog>
  );
}
