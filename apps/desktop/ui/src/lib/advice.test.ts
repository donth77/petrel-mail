import { afterEach, describe, expect, it } from 'vitest';
import { adviceText } from './advice';
import { english, setLocale } from './strings';

describe('advice the engine words in English', () => {
  afterEach(() => setLocale('en'));

  it('is shown in the reader’s language', () => {
    setLocale('de');
    expect(adviceText(english('advice-no-such-host'))).toBe(
      'Dieser Servername ließ sich nicht finden. Prüfen Sie den Host.',
    );
  });

  it('keeps the server named that a connection test failed on', () => {
    setLocale('fr');
    const said = `${english('advice-half-smtp')} — ${english('advice-no-tls')}`;
    expect(adviceText(said)).toBe(
      'Sortant (SMTP) — La connexion chiffrée n’a pas pu être établie, alors Petrel s’est arrêté plutôt que de continuer en clair.',
    );
  });

  it('shows the server’s own words as they came', () => {
    setLocale('ja');
    expect(adviceText('NO [UNAVAILABLE] Server busy, try later')).toBe(
      'NO [UNAVAILABLE] Server busy, try later',
    );
    expect(adviceText(`${english('advice-half-imap')} — NO [UNAVAILABLE] busy`)).toBe(
      '受信 (IMAP) — NO [UNAVAILABLE] busy',
    );
  });

  it('is unchanged in English', () => {
    setLocale('en');
    const said = english('advice-signin-refused-gmail');
    expect(adviceText(said)).toBe(said);
  });
});
