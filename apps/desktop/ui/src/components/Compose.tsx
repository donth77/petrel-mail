import { useEffect, useRef, useState } from 'react';
import { Paperclip, X } from 'lucide-react';
import { fileSize } from '../lib/format';
import type { Attached } from '../lib/attachments';
import { Icon } from './Icon';
import { key } from '../lib/keys';
import { t } from '../lib/strings';

export type Draft = {
  to: string;
  cc: string;
  subject: string;
  body: string;
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
  onSaveDraft: () => void;
  onSendLater: () => void;
  onPopOut: () => void;
};

/** Splits a recipient field into addresses, forgiving the separators people
 *  actually type. Validation belongs at send, not between keystrokes: telling
 *  someone their address is wrong while they are halfway through typing it is
 *  how a form gets in the way of being filled in. */
export function addresses(field: string): string[] {
  return field
    .split(/[,;]/)
    .map((a) => a.trim())
    .filter(Boolean);
}

/**
 * The docked composer.
 *
 * Docked rather than a separate window: a reply is a response to something you
 * are reading, and taking over the screen to write two lines loses the thing
 * being replied to. Popping out is a deliberate escalation, not the default.
 */
export function Compose({ draft, account, onChange, onClose, onSend, onAttach, onSaveDraft, onSendLater, onPopOut }: Props) {
  const toRef = useRef<HTMLInputElement>(null);
  const bodyRef = useRef<HTMLTextAreaElement>(null);
  const [showCc, setShowCc] = useState(draft.cc.length > 0);

  // Focus where the work is: a fresh message needs a recipient, a reply already
  // has one and needs words.
  useEffect(() => {
    if (draft.to) bodyRef.current?.focus();
    else toRef.current?.focus();
    // Once, on open — moving focus as the draft changes would fight the typist.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const field = <K extends keyof Draft>(k: K, v: Draft[K]) => onChange({ ...draft, [k]: v });

  return (
    <section
      className="compose"
      aria-label={t('compose-title')}
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
        <span className="clip">{account}</span>
      </div>

      <div className="hdrow">
        <span className="lab">{t('compose-to')}</span>
        <input
          ref={toRef}
          className="compose-input"
          value={draft.to}
          onChange={(e) => field('to', e.target.value)}
          aria-label={t('compose-to')}
          autoComplete="off"
          spellCheck={false}
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
          <input
            className="compose-input"
            value={draft.cc}
            onChange={(e) => field('cc', e.target.value)}
            aria-label={t('compose-cc')}
            autoComplete="off"
            spellCheck={false}
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

      <textarea
        ref={bodyRef}
        className="compose-body"
        value={draft.body}
        onChange={(e) => field('body', e.target.value)}
        aria-label={t('compose-body')}
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
