import { useEffect, useRef, useState } from 'react';
import { api } from '../lib/api';

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
  const frameRef = useRef<HTMLIFrameElement>(null);

  useEffect(() => {
    let live = true;
    setUrl(null);
    api
      .messageUrl(messageId)
      .then((u) => live && setUrl(u || null))
      .catch(() => live && setUrl(null));
    return () => {
      live = false;
    };
  }, [messageId]);

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

  if (!url) return <div className="body-loading" />;
  return (
    <iframe
      ref={frameRef}
      className="msg-frame"
      src={url}
      sandbox="allow-scripts"
      title={title}
      style={{ height }}
      scrolling="no"
    />
  );
}
