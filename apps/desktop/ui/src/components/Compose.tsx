import { useEffect, useRef, useState } from 'react';
import { Paperclip, X } from 'lucide-react';
import { fileSize } from '../lib/format';
import type { Attached } from '../lib/attachments';
import { Icon } from './Icon';
import { Recipients } from './Recipients';
import { RichText } from './RichText';
import { plainTextFromDoc } from '../lib/plain-text';
import { key } from '../lib/keys';
import { t } from '../lib/strings';

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
  onSend: () => void;
  onAttach: () => void;
  /** Files dragged in from the desktop. Separate from `onAttach`, which opens
      the picker: these arrive as bytes and have to be written down first. */
  onDropFiles: (files: FileList) => void;
  onSaveDraft: () => void;
  onSendLater: () => void;
  onPopOut: () => void;
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
/**
 * Whether a drag is carrying files from outside the application.
 *
 * Checked on every drag event because the composer sits over a window where
 * conversations are also being dragged about, and it must not light up for one
 * of those — nor swallow it. `types` is the only thing readable mid-drag; the
 * files themselves are withheld until the drop.
 */
function hasFiles(dt: DataTransfer | null): boolean {
  return !!dt && Array.from(dt.types).includes('Files');
}

export function Compose({ draft, account, onChange, onClose, onSend, onAttach, onDropFiles, onSaveDraft, onSendLater, onPopOut }: Props) {
  // Whether a file is being dragged over the composer. Counted rather than
  // toggled: dragging across a child element fires leave-then-enter, and a
  // boolean flickers off every time the pointer crosses an input.
  const [dragDepth, setDragDepth] = useState(0);
  const toRef = useRef<HTMLInputElement>(null);
  const [showCc, setShowCc] = useState(draft.cc.length > 0);

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
      aria-label={t('compose-title')}
      data-dropping={dragDepth > 0 || undefined}
      onDragEnter={(e) => {
        if (!hasFiles(e.dataTransfer)) return;
        e.preventDefault();
        setDragDepth((n) => n + 1);
      }}
      onDragOver={(e) => {
        // Cancelling is what makes a drop possible at all: the default answer
        // to a dragged file is to refuse it and let the OS open it instead.
        if (!hasFiles(e.dataTransfer)) return;
        e.preventDefault();
        e.dataTransfer.dropEffect = 'copy';
      }}
      onDragLeave={(e) => {
        if (!hasFiles(e.dataTransfer)) return;
        setDragDepth((n) => Math.max(0, n - 1));
      }}
      onDrop={(e) => {
        if (!hasFiles(e.dataTransfer)) return;
        e.preventDefault();
        setDragDepth(0);
        if (e.dataTransfer.files.length > 0) onDropFiles(e.dataTransfer.files);
      }}
      onKeyDown={(e) => {
        if ((e.metaKey || e.ctrlKey) && !e.shiftKey && e.key === 'Enter') {
          e.preventDefault();
          e.stopPropagation();
          onSend();
        }
        if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key === 'Enter') {
          e.preventDefault();
          e.stopPropagation();
          onSendLater();
          return;
        }
        if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key.toLowerCase() === 'o') {
          e.preventDefault();
          e.stopPropagation();
          onPopOut();
          return;
        }
        if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 's') {
          e.preventDefault();
          e.stopPropagation();
          onSaveDraft();
        }
      }}
    >
      {/* Shown over the composer rather than in it, so nothing shifts as the
          pointer arrives and the fields stay where they were aimed at. It takes
          no pointer events of its own — it must not become the thing the drop
          lands on, or the section beneath would never hear it. */}
      {dragDepth > 0 && (
        <div className="compose-drop" aria-hidden="true">
          <span>{t('compose-drop')}</span>
        </div>
      )}

      <header className="compose-head">
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
        />
        {!showCc && (
          <button type="button" className="compose-cc-toggle" onClick={() => setShowCc(true)}>
            {t('compose-cc')}
          </button>
        )}
      </div>

      {showCc && (
        <div className="hdrow">
          <span className="lab">{t('compose-cc')}</span>
          <Recipients label={t('compose-cc')} value={draft.cc} onChange={(v) => field('cc', v)} />
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
        autoFocus={Boolean(draft.to)}
        onChange={(html, doc) => onChange({ ...draft, html, body: plainTextFromDoc(doc) })}
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
        <button type="button" className="reply primary" onClick={onSend}>
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
