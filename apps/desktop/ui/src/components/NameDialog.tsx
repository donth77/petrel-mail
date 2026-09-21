import { useEffect, useId, useRef, useState } from 'react';
import { Dialog, DialogDismiss } from '@ariakit/react';
import { X } from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import { Icon } from './Icon';
import { t } from '../lib/strings';

/**
 * A one-field dialog for naming something: a new folder or tag, a saved
 * search, or a new name for any of the three.
 *
 * The rail's own + buttons open it too, expanded or collapsed. Naming in place
 * looked like the better gesture until the row turned up somewhere other than
 * where the field had been.
 */
export function NameDialog({
  open,
  title,
  placeholder,
  icon,
  suggested,
  prefix,
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
  /** Written before the field and not part of what is typed: the parent a new
   *  subfolder goes inside. `onSubmit` still receives only the typed name. */
  prefix?: string;
  /** The verb on the button. Enter has always worked; a dialog with no visible
   *  way to say yes looks like a dialog that cannot be finished. */
  confirmLabel: string;
  onClose: () => void;
  onSubmit: (name: string) => void;
}) {
  const field = useRef<HTMLInputElement>(null);
  const prefixId = useId();
  // Held so the button knows whether there is anything to save. Enter reads the
  // field directly, as it always did.
  //
  // Synced rather than only initialised: this component stays mounted while
  // the dialog is closed, so the initial value was whatever the suggestion was
  // when the window started — empty — and the Save button sat disabled over a
  // field with a name already in it.
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
      // Unmounted when shut, so every opening starts with a fresh field. Kept
      // mounted, the field held the last name typed into it: the second new
      // folder opened already reading "Receipts", with Create greyed out
      // beside it.
      unmountOnHide
      backdrop={<div className="palette-scrim" onClick={onClose} />}
      className="picker name-dialog"
      aria-label={title}
    >
      <div className="picker-head">
        <Icon icon={icon} size={14} />
        {prefix && (
          <span className="name-dialog-prefix" id={prefixId} title={prefix}>
            {prefix}
          </span>
        )}
        <input
          ref={field}
          aria-describedby={prefix ? prefixId : undefined}
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
            // name is being typed: typing "e" should not archive.
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
