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
 * A link whose visible spelling and real destination differ.
 *
 * The attack this exists for: `аpple.com` typed with a Cyrillic а reads as
 * `apple.com` to a person and resolves somewhere else entirely. Browsers
 * defuse it by showing the punycode form; a mail client hands the URL to
 * the browser, so the moment to say something is before it goes.
 */
export type HomographRisk = {
  /** What the address looks like — the sender's spelling. */
  asTyped: string;
  /** What it actually resolves to, in the ASCII form DNS uses. */
  asPunycode: string;
  reason: 'mixed-script' | 'latin-lookalike';
};

/** Characters from other alphabets that pass for Latin letters at a glance.
 *  Not the whole of UTS #39 — the ones that carry the attack. */
const LATIN_LOOKALIKES =
  /[\u0430\u0435\u043e\u0440\u0441\u0445\u0443\u0456\u0458\u04bb\u0501\u051b\u0561\u03bf\u03b1\u03bd\u03c1\u0261\u1d0f]/u;

/** The authority exactly as the sender wrote it, before any normalising. */
function typedHost(raw: string): string | null {
  const after = raw.slice(raw.indexOf('://') + 3);
  const host = after.split(/[/?#]/)[0];
  const noUser = host.includes('@') ? host.slice(host.lastIndexOf('@') + 1) : host;
  return noUser.replace(/:\d+$/, '') || null;
}

/**
 * Whether this link's spelling is worth a question first.
 *
 * Null for the ordinary case, including honest international domains: a
 * hostname written entirely in one non-Latin script is somebody's real
 * address, and warning about it would be warning about the existence of
 * other languages. What earns a question is a name that *borrows* — Latin
 * mixed with another script, or another script's letters chosen because
 * they look Latin.
 */
export function homographRisk(href: string): HomographRisk | null {
  let url: URL;
  try {
    url = new URL(href);
  } catch {
    return null;
  }
  const asPunycode = url.hostname;
  // No punycode label means no non-ASCII: nothing can be disguised.
  if (!asPunycode.split('.').some((label) => label.startsWith('xn--'))) return null;

  const asTyped = typedHost(href) ?? asPunycode;
  const hasLatin = /\p{Script=Latin}/u.test(asTyped);
  const hasOther = /[\p{Script=Cyrillic}\p{Script=Greek}\p{Script=Armenian}]/u.test(asTyped);
  if (hasLatin && hasOther) return { asTyped, asPunycode, reason: 'mixed-script' };
  if (LATIN_LOOKALIKES.test(asTyped)) return { asTyped, asPunycode, reason: 'latin-lookalike' };
  return null;
}

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
export function useMessageLinks(
  onMailto: (addr: string) => void,
  /** Asked before opening a link whose spelling disguises where it goes.
   *  Without a handler the link simply opens, which is the behaviour every
   *  caller had before this existed. */
  onRisky?: (risk: HomographRisk, open: () => void) => void,
) {
  useEffect(() => {
    function onMessage(e: MessageEvent) {
      const href = (e.data as { petrelOpen?: unknown })?.petrelOpen;
      if (typeof href !== 'string') return;
      const link = classifyLink(href);
      if (link.kind === 'web') {
        const risk = homographRisk(link.url);
        if (risk && onRisky) {
          onRisky(risk, () => void api.openExternal(link.url));
          return;
        }
        void api.openExternal(link.url);
      } else if (link.kind === 'mail') onMailto(link.addr);
    }
    window.addEventListener('message', onMessage);
    return () => window.removeEventListener('message', onMessage);
  }, [onMailto, onRisky]);
}
