import { useEffect, useRef } from 'react';
import { Dialog, DialogDismiss } from '@ariakit/react';
import { clickAway } from '../lib/dialog';
import { t } from '../lib/strings';

type Props = {
  open: boolean;
  title: string;
  /** What will actually happen, in the user's terms. Not a restatement of the
   *  title — if it says nothing the title did not, leave it out. */
  detail?: string | null;
  /** The verb, on the button. "Delete", not "OK": a button labelled OK makes
   *  people read the prose to find out what they are agreeing to. */
  confirmLabel: string;
  onConfirm: () => void;
  onClose: () => void;
};

/**
 * The dialog that stands in front of something irreversible.
 *
 * Petrel confirms almost nothing — undo is the better answer nearly every time,
 * and a client that asks "are you sure" after every gesture trains people to
 * dismiss it without reading, which is worse than not asking. This exists for
 * the small set of actions that undo genuinely cannot cover.
 *
 * Focus lands on Cancel, not on the destructive button. Someone who hits Return
 * out of habit should get the safe outcome; the one who means it can Tab once
 * or click. For the same reason the destructive button is never the one Enter
 * finds by default.
 */
export function Confirm({ open, title, detail, confirmLabel, onConfirm, onClose }: Props) {
  const cancel = useRef<HTMLButtonElement>(null);

  // Ariakit keeps the dialog mounted and hidden, so this has to run on each
  // opening rather than on mount — otherwise focus is set once, for the first
  // thing ever confirmed, and never again.
  useEffect(() => {
    if (open) cancel.current?.focus();
  }, [open]);

  return (
    <Dialog
      open={open}
      onClose={onClose}
      className="confirm-backdrop"
      {...clickAway(onClose)}
      backdrop={<div className="palette-scrim" onClick={onClose} />}
      aria-label={title}
    >
      <div className="confirm" role="alertdialog">
        <div className="confirm-title">{title}</div>
        {detail && <p className="confirm-detail">{detail}</p>}
        <div className="confirm-foot">
          <DialogDismiss ref={cancel} className="reply">
            {t('cancel')}
          </DialogDismiss>
          <button type="button" className="reply danger" onClick={onConfirm}>
            {confirmLabel}
          </button>
        </div>
      </div>
    </Dialog>
  );
}
