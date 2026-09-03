/**
 * The quoted original at the bottom of a reply.
 *
 * Two conventions matter here, and both are about the message arriving usefully
 * somewhere else rather than looking right in this app:
 *
 * **`<blockquote type="cite">`** is what Apple Mail, Thunderbird and Outlook
 * look for when deciding what to collapse behind a "show quoted text" control.
 * A plain `<blockquote>` is styled as a quote and never folded, so a long
 * thread grows an unfoldable wall of its own history in every client.
 *
 * **The attribution line goes outside the quote, above it.** Inside, it becomes
 * part of what is folded away, and the reader loses the one sentence that says
 * whose words these are.
 */

import { t } from './strings';

/** Escapes text for placing inside HTML. */
function escape(text: string): string {
  return text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

/** "On 4 March 2026 at 14:12, Dana Wu wrote:" — the form every client uses.
 *
 *  The sentence comes from the bundle, not from here. These words go out in
 *  the message: someone writing in French sent a reply whose only English
 *  was the line Petrel added to it. The date is formatted with the same
 *  language, which is what `locale` has always been for. */
export function attribution(from: string, dateMs: number, locale?: string): string {
  const when = new Date(dateMs);
  const date = when.toLocaleDateString(locale, {
    day: 'numeric',
    month: 'long',
    year: 'numeric',
  });
  const time = when.toLocaleTimeString(locale, { hour: '2-digit', minute: '2-digit' });
  return t('quote-attribution', { date, time, who: from });
}

/**
 * A reply's starting body: room to write, the attribution, then the original.
 *
 * The empty paragraph at the top is where the caret goes. Without it the reply
 * begins immediately above the attribution line with nowhere obvious to type,
 * which is the single most irritating thing a composer can do.
 */
export function replyBody(
  signature: string,
  from: string,
  dateMs: number,
  originalHtml: string,
  locale?: string,
): string {
  const line = escape(attribution(from, dateMs, locale));
  return [
    '<p></p>',
    signature,
    `<p>${line}</p>`,
    `<blockquote type="cite">${originalHtml}</blockquote>`,
  ].join('');
}

/**
 * A forward's starting body.
 *
 * Deliberately *not* `<blockquote type="cite">`, which is what a reply uses.
 * That markup asks the receiving client to fold the quoted text away behind a
 * "show quoted text" control — right for a reply, where the history is context
 * beneath your new sentence, and wrong for a forward, where the forwarded
 * message is the entire point and arriving collapsed hides it.
 *
 * The header block is the form every client writes and every client recognises,
 * which is what lets a forwarded message still read as one after it has been
 * forwarded on again.
 */
export function forwardBody(
  signature: string,
  from: string,
  to: string,
  subject: string,
  dateMs: number,
  originalHtml: string,
  locale?: string,
): string {
  const when = new Date(dateMs);
  const date = when.toLocaleDateString(locale, {
    weekday: 'short',
    day: 'numeric',
    month: 'short',
    year: 'numeric',
  });
  const time = when.toLocaleTimeString(locale, { hour: '2-digit', minute: '2-digit' });
  const lines = [
    `${t('quote-from')} ${escape(from)}`,
    `${t('quote-date')} ${escape(`${date} ${time}`)}`,
    `${t('quote-subject')} ${escape(subject)}`,
    // Omitted rather than left blank when the original had no visible
    // recipients — a header line reading "To:" with nothing after it looks
    // like the forward lost something.
    ...(to.trim() ? [`${t('quote-to')} ${escape(to)}`] : []),
  ];
  return [
    '<p></p>',
    signature,
    `<p>${escape(t('quote-forwarded'))}<br>${lines.join('<br>')}</p>`,
    originalHtml,
  ].join('');
}
