import { afterEach, describe, expect, it } from 'vitest';
import { attribution, forwardBody, replyBody } from './quote';
import { setLocale } from './strings';

const WHEN = Date.UTC(2026, 2, 4, 14, 12);

describe('attribution', () => {
  it('names who wrote and when', () => {
    const line = attribution('Dana Wu', WHEN, 'en-GB');
    expect(line).toContain('Dana Wu');
    expect(line).toContain('2026');
    expect(line).toMatch(/wrote:$/);
  });

  /* "01:05 PM" is what two-digit hours wrote, and no mail client does. A
     local time, so the check holds in any time zone the tests run in. */
  it('writes the hour as the language does, without a leading zero', () => {
    const afternoon = new Date(2026, 2, 4, 13, 5).getTime();
    const line = attribution('Dana Wu', afternoon, 'en-US');
    expect(line).toContain('1:05');
    expect(line).not.toContain('01:05');
    expect(attribution('Dana Wu', afternoon, 'en-GB')).toContain('13:05');
  });
});

describe('replyBody', () => {
  // A signature as startingHtml writes it: the caret line, a blank one, the
  // separator, then the signature's own lines.
  const SIG = '<p></p><p></p><p>-- </p><p>Sam</p>';
  const body = replyBody(SIG, 'Dana Wu', WHEN, '<p>The original.</p>', 'en-GB');

  /* Apple Mail, Thunderbird and Outlook fold on type="cite" specifically. A
     bare blockquote is styled as a quote and never collapsed, so every reply
     in a long thread carries an unfoldable copy of the whole history. */
  it('marks the quote as a citation so clients can collapse it', () => {
    expect(body).toContain('<blockquote type="cite">');
  });

  it('puts the attribution above the quote, not inside it', () => {
    const line = body.indexOf('Dana Wu');
    const quote = body.indexOf('<blockquote');
    expect(line).toBeGreaterThan(-1);
    expect(line).toBeLessThan(quote);
  });

  it('opens with somewhere to write', () => {
    expect(body.startsWith('<p></p>')).toBe(true);
    expect(replyBody('', 'Dana Wu', WHEN, '<p>x</p>', 'en-GB').startsWith('<p></p>')).toBe(true);
  });

  /* A paragraph is one line with no gap under it, so the space between what
     you write and the attribution has to be a blank line of its own. */
  it('leaves one blank line above the attribution, with or without a signature', () => {
    expect(body).toContain('<p>Sam</p><p></p><p>On ');
    const bare = replyBody('', 'Dana Wu', WHEN, '<p>x</p>', 'en-GB');
    expect(bare.startsWith('<p></p><p></p><p>On ')).toBe(true);
  });

  it('keeps the original inside the quote', () => {
    expect(body).toContain('<blockquote type="cite"><p>The original.</p></blockquote>');
  });

  /* The sender's name is not markup, however much it may look like it. */
  it('escapes a name that contains angle brackets', () => {
    const nasty = replyBody('', 'Dana <script>alert(1)</script>', WHEN, '<p>x</p>');
    expect(nasty).not.toContain('<script>');
    expect(nasty).toContain('&lt;script&gt;');
  });
});

describe('forwardBody', () => {
  const html = '<p>The original.</p>';

  it('does not fold the forwarded message away', () => {
    // A reply's quote is context and may collapse; a forward's content is the
    // message itself, and `type="cite"` would arrive hidden.
    const out = forwardBody('', 'Dana <d@e.example>', 'Sam <s@e.example>', 'Q3', 0, html, 'en-GB');
    expect(out).not.toContain('blockquote');
    expect(out).toContain('The original.');
  });

  it('writes the header block every client recognises', () => {
    const out = forwardBody('', 'Dana <d@e.example>', 'Sam <s@e.example>', 'Q3', 0, html, 'en-GB');
    expect(out).toContain('---------- Forwarded message ----------');
    expect(out).toContain('From: Dana');
    expect(out).toContain('Subject: Q3');
    expect(out).toContain('To: Sam');
  });

  /* The header block stands apart from what is written above it and from the
     message under it, as every client lays a forward out. */
  it('sets the header block apart with a blank line each side', () => {
    const out = forwardBody('', 'Dana', '', 'Q3', 0, html, 'en-GB');
    expect(out.startsWith('<p></p><p></p><p>---------- Forwarded message ----------')).toBe(true);
    expect(out).toContain('</p><p></p><p>The original.</p>');
  });

  it('leaves out a To line the original did not have', () => {
    const out = forwardBody('', 'Dana', '   ', 'Q3', 0, html, 'en-GB');
    expect(out).not.toContain('To:');
  });

  it('escapes the header values', () => {
    const out = forwardBody('', '<script>x</script>', '', 'a & b', 0, html, 'en-GB');
    expect(out).not.toContain('<script>');
    expect(out).toContain('a &amp; b');
  });
});

/* These words leave the machine. Somebody writing in French sent replies
   whose only English was the line Petrel put at the top of the quote, and a
   forward whose header block was English over a French message. */
describe('the quoted headers speak the language the user chose', () => {
  afterEach(() => setLocale('en'));

  it('writes the attribution in that language', () => {
    setLocale('fr');
    const line = attribution('Dana Wu', WHEN, 'fr');
    expect(line).toContain('a écrit');
    expect(line).not.toContain('wrote');
    expect(line).toContain('Dana Wu');
  });

  it('writes the forward header block in that language', () => {
    setLocale('de');
    const body = forwardBody('', 'Dana Wu', 'sam@example.com', 'Vertrag', WHEN, '<p>x</p>', 'de');
    expect(body).toContain('Weitergeleitete Nachricht');
    expect(body).toContain('Von:');
    expect(body).toContain('Betreff:');
    expect(body).not.toContain('Forwarded message');
  });

  it('still says it in English for an English reader', () => {
    setLocale('en');
    expect(attribution('Dana Wu', WHEN, 'en-GB')).toMatch(/wrote:$/);
    expect(forwardBody('', 'D', '', 'S', WHEN, '', 'en-GB')).toContain('Forwarded message');
  });
});
