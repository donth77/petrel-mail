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

/** Escapes text for placing inside HTML. */
function escape(text: string): string {
  return text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

/** "On 4 March 2026 at 14:12, Dana Wu wrote:" — the form every client uses. */
export function attribution(from: string, dateMs: number, locale?: string): string {
  const when = new Date(dateMs);
  const date = when.toLocaleDateString(locale, {
    day: 'numeric',
    month: 'long',
    year: 'numeric',
  });
  const time = when.toLocaleTimeString(locale, { hour: '2-digit', minute: '2-digit' });
  return `On ${date} at ${time}, ${from} wrote:`;
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
