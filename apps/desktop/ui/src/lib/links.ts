import { useEffect, useState } from 'react';
import { api } from './api';

/**
 * What clicking a link inside a message does.
 *
 * A message body is a sandboxed frame with no navigation of its own: it catches
 * the click and posts the destination out, and this decides what opening it
 * means. Keeping that decision here rather than in the frame is the point — the
 * frame renders sender-controlled markup, so it is the last place that should
 * be trusted to say what a link is.
 *
 * Web links go to the browser, because a mail client is not one: rendering a
 * linked page inside the reading pane would put a live, sender-chosen document
 * where the user expects their mail, and the pane's whole defence is that it
 * never does that.
 *
 * `mailto:` stays in Petrel. Handing it to the system would open whichever
 * other mail program the machine happens to prefer, which is a strange answer
 * from the mail program you are already reading in.
 */
export type Link =
  | { kind: 'web'; url: string }
  | { kind: 'mail'; addr: string }
  | { kind: 'blocked' };

/**
 * Reads a link's destination.
 *
 * An allowlist, not a blocklist. `file:` reaches local content, `javascript:`
 * executes, and the custom schemes other applications register are a wide and
 * unaudited surface for a stranger to aim at — so anything unrecognised is
 * simply not a link we open.
 */
export function classifyLink(href: string): Link {
  const raw = href.trim();
  const scheme = raw.slice(0, raw.indexOf(':') + 1).toLowerCase();
  if (scheme === 'http:' || scheme === 'https:') return { kind: 'web', url: raw };
  if (scheme === 'mailto:') {
    // `mailto:a@b.example?subject=hi` — the address is what precedes the query,
    // and it arrives percent-encoded often enough to be worth decoding.
    const body = raw.slice('mailto:'.length).split('?')[0];
    let addr: string;
    try {
      addr = decodeURIComponent(body);
    } catch {
      addr = body;
    }
    addr = addr.trim();
    return addr ? { kind: 'mail', addr } : { kind: 'blocked' };
  }
  return { kind: 'blocked' };
}

/**
 * The destination of whatever link is under the pointer, or null.
 *
 * Shown because the link opens in a browser, so there is no address bar on the
 * way to check it against — and because link text that disagrees with its
 * destination is the whole of phishing. The reader is entitled to look first.
 */
export function useHoveredLink(): string | null {
  const [href, setHref] = useState<string | null>(null);
  useEffect(() => {
    function onMessage(e: MessageEvent) {
      const url = (e.data as { petrelHover?: unknown })?.petrelHover;
      if (typeof url !== 'string') return;
      setHref(url || null);
    }
    window.addEventListener('message', onMessage);
    return () => window.removeEventListener('message', onMessage);
  }, []);
  return href;
}

/**
 * Listens for the link clicks a message frame forwards.
 *
 * Registered once per window rather than per message: the frames post to the
 * same parent, and one listener holding the policy beats the same decision
 * copied down through every component that happens to render a body.
 */
export function useMessageLinks(onMailto: (addr: string) => void) {
  useEffect(() => {
    function onMessage(e: MessageEvent) {
      const href = (e.data as { petrelOpen?: unknown })?.petrelOpen;
      if (typeof href !== 'string') return;
      const link = classifyLink(href);
      if (link.kind === 'web') void api.openExternal(link.url);
      else if (link.kind === 'mail') onMailto(link.addr);
    }
    window.addEventListener('message', onMessage);
    return () => window.removeEventListener('message', onMessage);
  }, [onMailto]);
}
