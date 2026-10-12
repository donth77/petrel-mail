import { english, t, type StringId } from './strings';

/** Advice the engine words, by the ids it uses (`ADVICE` in diag.rs). */
const ADVICE: StringId[] = [
  'advice-access-refused-gmail',
  'advice-access-refused',
  'advice-signin-refused-gmail',
  'advice-signin-refused-microsoft',
  'advice-signin-refused',
  'advice-no-such-host',
  'advice-no-answer-imap',
  'advice-no-answer-smtp',
  'advice-no-tls',
  'advice-unparsable',
  'advice-unreadable',
  'advice-no-folder-synced',
];

/** Which server a connection test failed on, as it heads the advice. */
const HALVES: StringId[] = ['advice-half-imap', 'advice-half-smtp'];

/**
 * What the engine said about a failed sign-in or sync, in the interface's
 * language.
 *
 * The engine words its advice in English, and these were the English shown
 * in every locale — on the screen a new account first fails on. Its sentences
 * are en.ftl's, word for word (a test in diag.rs holds the two equal), so a
 * sentence is known by its English and shown in the reader's own. Anything
 * that is not one of them is the server's own words, and is shown as it came.
 * The half that heads a connection test's advice is Petrel's own, and is
 * translated whatever follows it.
 */
export function adviceText(said: string): string {
  for (const half of HALVES) {
    const head = `${english(half)} — `;
    if (said.startsWith(head)) {
      return t('advice-with-half', { half: t(half), advice: adviceText(said.slice(head.length)) });
    }
  }
  const known = ADVICE.find((id) => english(id) === said);
  return known ? t(known) : said;
}
