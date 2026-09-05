import { useEffect, useRef, useState } from 'react';
import { api } from '../lib/api';
import { useSettings } from '../lib/settings';
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
  const { settings } = useSettings();
  const [url, setUrl] = useState<string | null>(null);
  // Set when the body could not be asked for. Distinct from "not yet": a
  // failure used to leave `url` null, which is the loading state, so a
  // message whose body could not be fetched showed a placeholder for ever.
  const [failed, setFailed] = useState<string | null>(null);
  const [height, setHeight] = useState(180);
  const [blocked, setBlocked] = useState(0);
  const [sender, setSender] = useState<string | null>(null);
  const frameRef = useRef<HTMLIFrameElement>(null);
  // Bumped to re-fetch the body once the policy for it has changed. The URL is
  // single-use, so a new one is the only way to render the same message again.
  const [reload, setReload] = useState(0);
  // The per-message escape from the dark transform (and from a sender's own
  // dark styling): render this one light. Session-local by intent.
  const [forceLight, setForceLight] = useState(false);
  // The system's answer, watched rather than read once. The frame is born
  // with the theme baked into its URL, so a flip from light to dark left
  // every open message on the old one until it was closed and reopened.
  const [systemDark, setSystemDark] = useState(
    () => typeof window.matchMedia === 'function'
      && window.matchMedia('(prefers-color-scheme: dark)').matches,
  );
  useEffect(() => {
    if (typeof window.matchMedia !== 'function') return;
    const mq = window.matchMedia('(prefers-color-scheme: dark)');
    const onChange = (e: MediaQueryListEvent) => setSystemDark(e.matches);
    mq.addEventListener('change', onChange);
    return () => mq.removeEventListener('change', onChange);
  }, []);
  const appDark =
    settings.theme === 'dark' || (settings.theme !== 'light' && systemDark);

  useEffect(() => {
    let live = true;
    setUrl(null);
    setFailed(null);
    setBlocked(0);
    api
      .messageUrl(messageId)
      // The app's theme rides the URL so the frame is *born* the right color
      // — anything pushed in after first paint is a white flash on every
      // message open. Which messages may actually go dark is the frame's own
      // decision (sender-declared, or plain text); this only says what the
      // app looks like today.
      .then((u) => {
        if (!live) return;
        // Resolved to light/dark here: the frame's transform has to decide
        // *now*, and "system" is only answerable on this side of the wall.
        const resolved = appDark ? 'dark' : 'light';
        const force = forceLight ? '&force=light' : '';
        setUrl(u ? `${u}${u.includes('?') ? '&' : '?'}theme=${resolved}${force}` : null);
      })
      .catch((e) => {
        if (!live) return;
        setUrl(null);
        setFailed(String(e));
      });
    return () => {
      live = false;
    };
  }, [messageId, reload, settings.theme, forceLight, appDark]);

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
      if (typeof b === 'number') setBlocked((prev) => (prev === b ? prev : b));

      const h = data?.petrelHeight;
      if (typeof h === 'number' && h > 0 && h < 20000) {
        const ceil = Math.ceil(h);
        setHeight((prev) => (prev === ceil ? prev : ceil));
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

  // The reading-size preference, sent in rather than inherited.
  //
  // The frame is opaque-origin, so a CSS variable on the host cannot reach it —
  // which is why setting this appeared to do nothing at all: it was styling the
  // container around a frame whose own stylesheet said 14px. Sent on every
  // change as well as on load, so the size moves while the setting is being
  // tried rather than on the next message opened.
  useEffect(() => {
    const n = Number(settings.readingTextSize);
    if (!Number.isFinite(n)) return;
    const send = () => frameRef.current?.contentWindow?.postMessage({ petrelSize: n }, '*');
    send();
    // Again once the frame has parsed its script, which may not have run yet
    // when the size changes at the same moment the body loads.
    const again = setTimeout(send, 120);
    return () => clearTimeout(again);
  }, [settings.readingTextSize, url]);

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

  if (failed) {
    return (
      <div className="body-failed">
        <p>{t('msg-body-failed')}</p>
        <p className="mono">{failed}</p>
      </div>
    );
  }
  if (!url) return <div className="body-loading" />;
  return (
    <>
      {blocked > 0 && (
        <div className="blocked-remote">
          <span className="blocked-what">
            {t('remote-blocked', { count: blocked })}
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
      {appDark && (
        <div className="frame-theme-row">
          <button
            type="button"
            className="linkish"
            onClick={() => setForceLight((v) => !v)}
          >
            {forceLight ? t('msg-view-dark') : t('msg-view-light')}
          </button>
        </div>
      )}
    {/* No `scrolling="no"`: that turns off both axes, and sideways is the last
        resort for a message too wide to shrink to a readable size — without it
        such a message is simply cut off with no way to reach the rest. The
        frame still never scrolls vertically; the document's own CSS holds that,
        and the height below is what makes it unnecessary. */}
    <iframe
      ref={frameRef}
      className="msg-frame"
      src={url}
      sandbox="allow-scripts"
      title={title}
      style={{ height }}
    />
    </>
  );
}
