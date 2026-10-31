import { useEffect, useRef, useState } from 'react';
import { Paperclip, X } from 'lucide-react';
import { fileSize } from '../lib/format';
import type { Attached, Copying } from '../lib/attachments';
import { Icon } from './Icon';
import { Recipients, type RecipientsHandle } from './Recipients';
import { RichText, type RichTextHandle } from './RichText';
import { plainTextFromDoc } from '../lib/plain-text';
import { isBccKey, key } from '../lib/keys';
import { t } from '../lib/strings';
import { useDragWindow } from '../lib/drag-window';
import { useFileDropZone } from '../lib/useFileDrop';
import { dialogOpen } from '../lib/useKeyboard';

export type Draft = {
  to: string;
  cc: string;
  /** Blind copies: on the envelope and in the sender's own copies, and in no
   *  header anyone else sees. Absent on a message from before there was one,
   *  which is the same as empty. */
  bcc?: string;
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
  /** Dropped files whose bytes are still being copied. Shown at once; the
   *  saved row learns the real path when the copy finishes. */
  copying?: Copying[];
  /** Set once saved, so saving again updates rather than multiplying. */
  savedId?: number | null;
  /** The account the message is from, when that is not the one on screen:
   *  a held send pulled back from the bar after a switch. Its From line,
   *  and the hold on its next send, are that account's. */
  account?: number | null;
};

/** Merges a patch into the draft the updater sees, not a snapshot from the last render. */
export function patchDraft(current: Draft, patch: Partial<Draft>): Draft {
  // A body edit used to copy the draft from the last render, and an attachment
  // added after that render disappeared. The updater receives the draft as it is now.
  return { ...current, ...patch };
}

type Props = {
  draft: Draft;
  account: string;
  onChange: (update: (current: Draft) => Draft) => void;
  onClose: () => void;
  /** These three are handed the draft as it stands at the keystroke, with any
   *  recipient still being typed committed. The parent's own copy is a render
   *  behind by then, and a send read from it went out without that address. */
  onSend: (d: Draft) => void;
  onAttach: () => void;
  /** Files dragged in from the desktop. Separate from `onAttach`, which opens
      the picker: these arrive as bytes and have to be written down first. */
  onDropFiles: (files: readonly File[]) => void;
  /** Takes a chip back while its copy is still running, so the copy is not
   *  kept when it finishes. */
  onRemoveCopying: (id: string) => void;
  onSaveDraft: (d: Draft) => void;
  /** Handed the draft as it stands at the keystroke, as Send is, so it can
   *  say there is nobody to send to before a time is asked for. */
  onSendLater: (d: Draft) => void;
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
export function Compose({ draft, account, onChange, onClose, onSend, onAttach, onDropFiles, onRemoveCopying, onSaveDraft, onSendLater, onPopOut, onNotice, pane }: Props) {
  const { over: dropping, dropProps } = useFileDropZone(onDropFiles);
  const toRef = useRef<HTMLInputElement>(null);
  const ccRef = useRef<HTMLInputElement>(null);
  const bccRef = useRef<HTMLInputElement>(null);
  const toField = useRef<RecipientsHandle>(null);
  const ccField = useRef<RecipientsHandle>(null);
  const bccField = useRef<RecipientsHandle>(null);
  const [showCc, setShowCc] = useState(draft.cc.length > 0);
  const body = useRef<RichTextHandle | null>(null);
  // The field the person was last writing in, so a button that takes focus —
  // or, in WebKit, drops it to the page — can hand it back.
  const lastField = useRef<HTMLElement | null>(null);
  // Open whenever the message has blind copies, as when a draft that has
  // some is resumed; never closed under someone, since they may be typing in it.
  const [showBcc, setShowBcc] = useState((draft.bcc ?? '').length > 0);
  if (!showBcc && (draft.bcc ?? '').length > 0) setShowBcc(true);
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
    const bcc = bccField.current?.flush() ?? null;
    if (to == null && cc == null && bcc == null) return draft;
    const next = { ...draft, to: to ?? draft.to, cc: cc ?? draft.cc, bcc: bcc ?? draft.bcc };
    onChange((current) =>
      patchDraft(current, {
        to: to ?? current.to,
        cc: cc ?? current.cc,
        bcc: bcc ?? current.bcc,
      }),
    );
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
  // The same for Bcc, whether asked for by its button or by its keys.
  const bccAsked = useRef(false);
  useEffect(() => {
    if (showBcc && bccAsked.current) {
      bccAsked.current = false;
      bccRef.current?.focus();
    }
  }, [showBcc]);
  const askForBcc = () => {
    if (showBcc) {
      bccRef.current?.focus();
      return;
    }
    bccAsked.current = true;
    setShowBcc(true);
  };
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

  /** The composer's own shortcuts. True when the key was one of them. */
  const composerKey = (e: {
    metaKey: boolean;
    ctrlKey: boolean;
    shiftKey: boolean;
    key: string;
    preventDefault: () => void;
  }): boolean => {
    if (!(e.metaKey || e.ctrlKey)) return false;
    if (!e.shiftKey && e.key === 'Enter') {
      e.preventDefault();
      onSend(settled());
      return true;
    }
    if (e.shiftKey && e.key === 'Enter') {
      e.preventDefault();
      // Handed the settled draft, so the recipients can be checked before a
      // time is asked for. The picker reads the draft again when a time is
      // chosen, by which point the commit has landed.
      onSendLater(settled());
      return true;
    }
    if (e.shiftKey && e.key.toLowerCase() === 'o') {
      e.preventDefault();
      onPopOut(settled());
      return true;
    }
    if (e.key.toLowerCase() === 's') {
      e.preventDefault();
      onSaveDraft(settled());
      return true;
    }
    return false;
  };
  const keysRef = useRef(composerKey);
  keysRef.current = composerKey;

  // The same keys when focus has strayed out of the composer onto the page —
  // as a click on Attach leaves it in WebKit — so ⌘↵ still sends what is
  // being written. Only from the page itself: a key pressed in the list, the
  // search field or a dialog belongs to that. ⌘⇧S there is the search's.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented) return;
      const at = document.activeElement;
      if (at && at !== document.body && at !== document.documentElement) return;
      if (dialogOpen()) return;
      if (e.shiftKey && e.key.toLowerCase() === 's') return;
      keysRef.current(e);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  /** Back to the field the person was writing in, or the body. */
  const backToWriting = () => {
    const el = lastField.current;
    if (el?.isConnected && !el.isContentEditable) el.focus();
    else body.current?.focus();
  };

  const field = <K extends keyof Draft>(k: K, v: Draft[K]) =>
    onChange((current) => patchDraft(current, { [k]: v }));

  return (
    <section
      className="compose"
      ref={pane ? undefined : (drag.ref as React.RefObject<HTMLElement>)}
      style={pane ? undefined : drag.style}
      // ⌥⌘B on the Mac, as in Apple Mail; Ctrl+Shift+B elsewhere, as in
      // Gmail, and never with AltGr held (`isBccKey`). Taken in the capture
      // phase, before the editor's own keys.
      onKeyDownCapture={(e) => {
        if (isBccKey(e)) {
          e.preventDefault();
          e.stopPropagation();
          askForBcc();
        }
      }}
      aria-label={t('compose-title')}
      data-pane={pane || undefined}
      data-dropping={dropping || undefined}
      {...dropProps}
      onKeyDown={(e) => {
        if (composerKey(e)) e.stopPropagation();
      }}
      onFocus={(e) => {
        const el = e.target as HTMLElement;
        if (el.isContentEditable || el.tagName === 'INPUT' || el.tagName === 'TEXTAREA') {
          lastField.current = el;
        }
      }}
    >
      {/* A sign only. It must not take the drop: WKWebView hands the file to
          the element the drag was already over, which is the body, and an
          overlay that intercepts it gets nothing — no name, no remove button. */}
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
        {!showBcc && (
          <button
            type="button"
            className="compose-cc-toggle"
            title={t('compose-bcc-hint', { key: key('bcc') })}
            onClick={askForBcc}
          >
            {t('compose-bcc')}
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

      {showBcc && (
        <div className="hdrow">
          <span className="lab">{t('compose-bcc')}</span>
          <Recipients
            label={t('compose-bcc')}
            value={draft.bcc ?? ''}
            onChange={(v) => field('bcc', v)}
            inputRef={bccRef}
            handle={bccField}
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
          onKeyDown={(e) => {
            // Into the message, as every mail client goes. The formatting
            // toolbar sits between the two, and Tab used to land on its font
            // menu, where the next letters typed were mail commands.
            if (e.key !== 'Tab' || e.shiftKey || e.metaKey || e.ctrlKey || e.altKey) return;
            if (!body.current) return;
            e.preventDefault();
            body.current.focusStart();
          }}
        />
      </div>

      {/* Both halves come out of one change: the HTML that is sent, and the
          text generated from the same document. Deriving one from the other
          later would mean two descriptions of one message that can disagree. */}
      <RichText
        handle={body}
        html={draft.html}
        autoFocus={bodyFocus}
        onChange={(html, doc) =>
          onChange((current) =>
            patchDraft(current, { html, body: plainTextFromDoc(doc) }),
          )
        }
        onDropFiles={onDropFiles}
        onNotice={onNotice}
      />

      {((draft.attachments?.length ?? 0) > 0 || (draft.copying?.length ?? 0) > 0) && (
        <div className="compose-files">
          {[
            ...(draft.attachments ?? []).map((a) => ({
              key: a.path,
              name: a.name,
              size: a.size,
              remove: () =>
                onChange((current) =>
                  patchDraft(current, {
                    attachments: (current.attachments ?? []).filter((x) => x.path !== a.path),
                  }),
                ),
            })),
            ...(draft.copying ?? []).map((a) => ({
              key: a.id,
              name: a.name,
              size: a.size,
              remove: () => onRemoveCopying(a.id),
            })),
          ].map((a) => (
            <span className="att-chip" key={a.key}>
              <Icon icon={Paperclip} size={11} />
              <span className="clip">{a.name}</span>
              <span className="mono att-size">{fileSize(a.size)}</span>
              <button
                type="button"
                className="att-remove"
                aria-label={t('compose-remove-attachment', { name: a.name })}
                onClick={a.remove}
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
        <button
          type="button"
          className="reply"
          onClick={() => {
            onAttach();
            // Back where the writing was. Chromium leaves focus on this
            // button and WebKit drops it to the page; either way the next
            // letters typed were not going into the message.
            backToWriting();
          }}
        >
          <Icon icon={Paperclip} size={14} />
          {t('compose-attach')}
        </button>
      </footer>
    </section>
  );
}
