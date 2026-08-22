/** Words that promise an attachment. Kept deliberately short: every extra
 *  pattern buys a few more catches and a lot more false alarms, and a warning
 *  that cries wolf is one people learn to dismiss without reading — at which
 *  point it stops catching the real case too. */
const PROMISES = [
  /\battach(ed|ing|ment|ments)?\b/i,
  /\benclosed\b/i,
  /\bsee the (file|doc|document|pdf|deck|spreadsheet)\b/i,
  /\bhere('|\u2019)?s the (file|doc|document|pdf|deck)\b/i,
];

/**
 * Whether a message promises an attachment it does not carry.
 *
 * Quoted text is stripped first: replying to someone who wrote "see attached"
 * is not a promise you made, and warning about their words is the fastest way
 * to make the feature annoying.
 */
export function promisesMissingAttachment(
  subject: string,
  body: string,
  attachmentCount: number,
): boolean {
  if (attachmentCount > 0) return false;
  const own = body
    .split('\n')
    .filter((line) => !line.trimStart().startsWith('>'))
    .join('\n');
  const text = `${subject}\n${own}`;
  return PROMISES.some((re) => re.test(text));
}
