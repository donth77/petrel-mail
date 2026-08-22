import { useEffect, useRef, useState } from 'react';
import { api } from '../lib/api';
import { t } from '../lib/strings';

/**
 * A message body, rendered in a sandboxed frame served over `petrel-msg:`.
 *
 * The frame sizes itself: it posts its height out, and we set the iframe to
 * match, so the conversation scrolls as one column instead of nesting a scroll
 * region inside a scroll region. That requires one script *we* inject, admitted
 * by a per-response CSP nonce. Sender script is stripped
 * by the sanitizer and could never carry a matching nonce anyway; the frame is
 * still opaque-origin, still network-blocked by CSP, still cut off from IPC.
 */
export function MessageBody({ messageId, title }: { messageId: number; title: string }) {
  const [url, setUrl] = useState<string | null>(null);
  const [height, setHeight] = useState(180);
  const [blocked, setBlocked] = useState(0);
  const [sender, setSender] = useState<string | null>(null);
  const frameRef = useRef<HTMLIFrameElement>(null);
  // Bumped to re-fetch the body once the policy for it has changed. The URL is
  // single-use, so a new one is the only way to render the same message again.
  const [reload, setReload] = useState(0);

  useEffect(() => {
    let live = true;
    setUrl(null);
    setBlocked(0);
    api
      .messageUrl(messageId)
      .then((u) => live && setUrl(u || null))
      .catch(() => live && setUrl(null));
    return () => {
      live = false;
    };
  }, [messageId, reload]);

  useEffect(() => {
    function onMessage(e: MessageEvent) {
      // Only accept the shape we defined, and only from our own frame: the
      // window is addressable by anything that can post to it.
      if (e.source !== frameRef.current?.contentWindow) return;
      const data = e.data as {
        petrelHeight?: unknown;
        petrelKey?: {
          key: string;
          metaKey: boolean;
          ctrlKey: boolean;
          shiftKey: boolean;
          altKey: boolean;
        };
      };

      // How much the sanitizer refused. Reported out rather than drawn inside
      // the frame: a banner in there could say what happened but never offer to
      // undo it — the frame has no IPC and no same-origin access by design.
      const b = (data as { petrelBlocked?: unknown })?.petrelBlocked;
      if (typeof b === 'number') setBlocked(b);

      const h = data?.petrelHeight;
      if (typeof h === 'number' && h > 0 && h < 20000) {
        setHeight(Math.ceil(h));
      }

      // A focused frame swallows keydown, so every shortcut in the app dies the
      // moment you click a message. The frame forwards key identity and we
      // replay it here, on the window the rest of the app listens to.
      const k = data?.petrelKey;
      if (k && typeof k.key === 'string') {
        window.dispatchEvent(
          new KeyboardEvent('keydown', {
            key: k.key,
            metaKey: !!k.metaKey,
            ctrlKey: !!k.ctrlKey,
            shiftKey: !!k.shiftKey,
            altKey: !!k.altKey,
            bubbles: true,
          }),
        );
      }
    }
    window.addEventListener('message', onMessage);
    return () => window.removeEventListener('message', onMessage);
  }, []);

  const allow = async (always: boolean) => {
    try {
      const addr = always ? await api.trustSender(messageId) : null;
      if (!always) await api.showRemoteOnce(messageId);
      if (addr) setSender(addr);
      setBlocked(0);
      setReload((n) => n + 1);
    } catch (e) {
      void api.log(`remote content: ${e}`);
    }
  };

  if (!url) return <div className="body-loading" />;
  return (
    <>
      {blocked > 0 && (
        <div className="blocked-remote">
          <span className="blocked-what">
            {t('remote-blocked', { count: String(blocked) })}
          </span>
          <button type="button" className="linkish" onClick={() => void allow(false)}>
            {t('remote-show-once')}
          </button>
          <button type="button" className="linkish" onClick={() => void allow(true)}>
            {t('remote-always')}
          </button>
        </div>
      )}
      {sender && <div className="blocked-remote quiet">{t('remote-trusted', { addr: sender })}</div>}
    <iframe
      ref={frameRef}
      className="msg-frame"
      src={url}
      sandbox="allow-scripts"
      title={title}
      style={{ height }}
      scrolling="no"
    />
    </>
  );
}
