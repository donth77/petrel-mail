import { useEffect, useRef, useState } from 'react';
import { Paperclip, X } from 'lucide-react';
import { fileSize } from '../lib/format';
import type { Attached } from '../lib/attachments';
import { Icon } from './Icon';
import { Recipients, type RecipientsHandle } from './Recipients';
import { RichText } from './RichText';
import { plainTextFromDoc } from '../lib/plain-text';
import { key } from '../lib/keys';
import { t } from '../lib/strings';
import { useDragWindow } from '../lib/drag-window';
import { useFileDropZone } from '../lib/useFileDrop';

export type Draft = {
  to: string;
  cc: string;
  subject: string;
  /** Plain text, generated from the editor's document. What the
   *  missing-attachment check reads and what goes out as the text half. */
  body: string;
  /** The rich half, as the editor produced it. */
  html: string;
  /** Set when this is a reply, so the thread survives at the other end. */
  inReplyTo?: string | null;
  references?: string[];
  attachments?: Attached[];
  /** Set once saved, so saving again updates rather than multiplying. */
  savedId?: number | null;
};

type Props = {
  draft: Draft;
  account: string;
  onChange: (d: Draft) => void;
  onClose: () => void;
  /** These three are handed the draft as it stands at the keystroke, with any
   *  recipient still being typed committed. The parent's own copy is a render
   *  behind by then, and a send read from it went out without that address. */
  onSend: (d: Draft) => void;
  onAttach: () => void;
  /** Files dragged in from the desktop. Separate from `onAttach`, which opens
      the picker: these arrive as bytes and have to be written down first. */
  onDropFiles: (files: FileList) => void;
  onSaveDraft: (d: Draft) => void;
  onSendLater: () => void;
  onPopOut: (d: Draft) => void;
  /** Passing notes to the toast — a refused paste, and nothing graver. */
  onNotice?: (text: string) => void;
  /** Fills the reading-pane slot instead of floating over it. */
  pane?: boolean;
};

/** Splits a recipient field into addresses.
 *
 * Re-exported rather than reimplemented: the chip field and the send path have
 * to agree about what counts as a recipient, and two copies of one rule is how
 * they stop agreeing. */
export { splitRecipients as addresses } from '../lib/recipients';

/**
 * The docked composer.
 *
 * Docked rather than a separate window: a reply is a response to something you
 * are reading, and taking over the screen to write two lines loses the thing
 * being replied to. Popping out is a deliberate escalation, not the default.
 */
export function Compose({ draft, account, onChange, onClose, onSend, onAttach, onDropFiles, onSaveDraft, onSendLater, onPopOut, onNotice, pane }: Props) {
  const { over: dropping, dropProps } = useFileDropZone(onDropFiles);
  const toRef = useRef<HTMLInputElement>(null);
  const ccRef = useRef<HTMLInputElement>(null);
  const toField = useRef<RecipientsHandle>(null);
  const ccField = useRef<RecipientsHandle>(null);
  const [showCc, setShowCc] = useState(draft.cc.length > 0);
  // Decided once, as the composer opens. It used to follow the draft, so
  // committing the first To recipient flipped it and the editor took focus
  // away from the field the person was still typing in.
  const [bodyFocus] = useState(() => Boolean(draft.to));

  /** The draft with anything still being typed in a recipient field made a
   *  recipient. Returned as well as reported, because the caller acts on it
   *  in the same keystroke. */
  const settled = (): Draft => {
    const to = toField.current?.flush() ?? null;
    const cc = ccField.current?.flush() ?? null;
    if (to == null && cc == null) return draft;
    const next = { ...draft, to: to ?? draft.to, cc: cc ?? draft.cc };
    onChange(next);
    return next;
  };
  // The Cc button unmounts itself when clicked, and focus fell to the body:
  // the next letters typed were single-key shortcuts, so "eve@" archived the
  // conversation being replied to and opened Move. Asking for the field
  // means wanting to type in it.
  const ccAsked = useRef(false);
  useEffect(() => {
    if (showCc && ccAsked.current) {
      ccAsked.current = false;
      ccRef.current?.focus();
    }
  }, [showCc]);
  // Draggable by its header. The pop-out button is still the way to get a
  // real OS window; this is for nudging it off whatever it is covering.
  // A pane composer already has a place; dragging it would leave the slot.
  const drag = useDragWindow();

  // Focus where the work is: a fresh message needs a recipient, a reply already
  // has one and needs words. The body half is the editor's own autoFocus, which
  // has to wait for it to exist.
  useEffect(() => {
    if (!draft.to) toRef.current?.focus();
    // Once, on open — moving focus as the draft changes would fight the typist.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const field = <K extends keyof Draft>(k: K, v: Draft[K]) => onChange({ ...draft, [k]: v });

  return (
    <section
      className="compose"
      ref={pane ? undefined : (drag.ref as React.RefObject<HTMLElement>)}
      style={pane ? undefined : drag.style}
      aria-label={t('compose-title')}
      data-pane={pane || undefined}
      data-dropping={dropping || undefined}
      {...dropProps}
      onKeyDown={(e) => {
        if ((e.metaKey || e.ctrlKey) && !e.shiftKey && e.key === 'Enter') {
          e.preventDefault();
          e.stopPropagation();
          onSend(settled());
        }
        if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key === 'Enter') {
          e.preventDefault();
          e.stopPropagation();
          // The picker reads the draft when a time is chosen, by which
          // point the commit above has landed.
          settled();
          onSendLater();
          return;
        }
        if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key.toLowerCase() === 'o') {
          e.preventDefault();
          e.stopPropagation();
          onPopOut(settled());
          return;
        }
        if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 's') {
          e.preventDefault();
          e.stopPropagation();
          onSaveDraft(settled());
        }
      }}
    >
      {/* Shown over the composer rather than in it, so nothing shifts as the
          pointer arrives and the fields stay where they were aimed at. It takes
          no pointer events of its own — it must not become the thing the drop
          lands on, or the section beneath would never hear it. */}
      {dropping && (
        <div className="compose-drop" aria-hidden="true">
          <span>{t('compose-drop')}</span>
        </div>
      )}

      <header className="compose-head" {...(pane ? {} : drag.handleProps)}>
        <span className="compose-title">{draft.inReplyTo ? t('compose-reply') : t('compose-new')}</span>
        {/* Closing keeps the message. Discarding what someone wrote because
            they hit the wrong corner is unforgivable, and a confirmation
            dialog for every close is worse than just keeping it. */}
        <button type="button" className="close-btn" onClick={onClose} aria-label={t('close')}>
          <Icon icon={X} size={15} />
        </button>
      </header>

      <div className="hdrow">
        <span className="lab">{t('compose-from')}</span>
        {/* Read-only, and shaped to say so rather than explained. In the same
            row as To and Subject it read as a field to click into; a filled
            chip reads as a value that was decided elsewhere. */}
        <span className="clip compose-from-value">{account}</span>
      </div>

      <div className="hdrow">
        <span className="lab">{t('compose-to')}</span>
        <Recipients
          label={t('compose-to')}
          value={draft.to}
          onChange={(v) => field('to', v)}
          inputRef={toRef}
          handle={toField}
        />
        {!showCc && (
          <button
            type="button"
            className="compose-cc-toggle"
            onClick={() => {
              ccAsked.current = true;
              setShowCc(true);
            }}
          >
            {t('compose-cc')}
          </button>
        )}
      </div>

      {showCc && (
        <div className="hdrow">
          <span className="lab">{t('compose-cc')}</span>
          <Recipients
            label={t('compose-cc')}
            value={draft.cc}
            onChange={(v) => field('cc', v)}
            inputRef={ccRef}
            handle={ccField}
          />
        </div>
      )}

      <div className="hdrow">
        <span className="lab">{t('compose-subject')}</span>
        <input
          className="compose-input"
          value={draft.subject}
          onChange={(e) => field('subject', e.target.value)}
          aria-label={t('compose-subject')}
        />
      </div>

      {/* Both halves come out of one change: the HTML that is sent, and the
          text generated from the same document. Deriving one from the other
          later would mean two descriptions of one message that can disagree. */}
      <RichText
        html={draft.html}
        autoFocus={bodyFocus}
        onChange={(html, doc) => onChange({ ...draft, html, body: plainTextFromDoc(doc) })}
        onNotice={onNotice}
      />

      {(draft.attachments?.length ?? 0) > 0 && (
        <div className="compose-files">
          {draft.attachments!.map((a) => (
            <span className="att-chip" key={a.path}>
              <Icon icon={Paperclip} size={11} />
              <span className="clip">{a.name}</span>
              <span className="mono att-size">{fileSize(a.size)}</span>
              <button
                type="button"
                className="att-remove"
                aria-label={t('compose-remove-attachment', { name: a.name })}
                onClick={() =>
                  onChange({
                    ...draft,
                    attachments: draft.attachments!.filter((x) => x.path !== a.path),
                  })
                }
              >
                <Icon icon={X} size={11} />
              </button>
            </span>
          ))}
        </div>
      )}

      <footer className="compose-foot">
        <button type="button" className="reply primary" onClick={() => onSend(settled())}>
          {t('compose-send')} <span className="kbd on-accent">{key('send')}</span>
        </button>
        <button type="button" className="reply" onClick={onAttach}>
          <Icon icon={Paperclip} size={14} />
          {t('compose-attach')}
        </button>
      </footer>
    </section>
  );
}
