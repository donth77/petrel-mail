import { useEffect, useState } from 'react';
import { api, type UnsubInfo } from '../lib/api';
import { Confirm } from './Confirm';
import { t } from '../lib/strings';

/**
 * The way out of a mailing list, offered where it is safe to take.
 *
 * The message's own List-Unsubscribe header is the sender's formal offer;
 * the "unsubscribe" link buried in the footer is a tracked link like every
 * other in the body. So the chrome offers the header's version: one-click
 * (RFC 8058) posts without opening anything, a plain URL opens the browser,
 * and a mailto-only offer composes the request for you to send.
 *
 * Confirmed first, always. It is a message *to the sender* about you, and a
 * single stray click should not send one.
 */
export function Unsubscribe({
  messageId,
  sender,
  onToast,
  onComposeMailto,
}: {
  messageId: number;
  sender: string;
  onToast: (text: string) => void;
  /** Absent where no composer can open (the popped-out reader). */
  onComposeMailto?: (to: string, subject: string) => void;
}) {
  const [info, setInfo] = useState<UnsubInfo | null>(null);
  const [asking, setAsking] = useState(false);

  useEffect(() => {
    let live = true;
    setInfo(null);
    api
      .unsubscribeInfo(messageId)
      .then((u) => live && setInfo(u))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [messageId]);

  if (!info) return null;
  // A mailto-only offer needs a composer to be worth anything.
  if (!info.one_click && !info.url && !onComposeMailto) return null;

  const detail = info.one_click
    ? t('unsub-body-oneclick', { sender })
    : info.url
      ? t('unsub-body-url', { sender })
      : t('unsub-body-mailto', { sender });

  const run = () => {
    setAsking(false);
    if (info.one_click) {
      void api
        .unsubscribeOneClick(messageId)
        .then(() => onToast(t('unsub-done', { sender })))
        .catch((e) => onToast(t('unsub-failed', { error: String(e) })));
      return;
    }
    if (info.url) {
      void api.openExternal(info.url).catch((e) => onToast(t('unsub-failed', { error: String(e) })));
      return;
    }
    if (info.mailto && onComposeMailto) {
      // mailto:leave@x.example?subject=unsubscribe — address and subject are
      // all a composer needs; anything else the sender packed in is theirs.
      const uri = info.mailto.slice('mailto:'.length);
      const [addr, query = ''] = uri.split('?');
      const subject =
        new URLSearchParams(query).get('subject') ?? 'unsubscribe';
      onComposeMailto(decodeURIComponent(addr), subject);
    }
  };

  return (
    <>
      <button type="button" className="unsub" onClick={() => setAsking(true)}>
        {t('unsub-button')}
      </button>
      <Confirm
        open={asking}
        title={t('unsub-confirm', { sender })}
        detail={detail}
        confirmLabel={t('unsub-button')}
        onClose={() => setAsking(false)}
        onConfirm={run}
      />
    </>
  );
}
