import { describe, expect, it } from 'vitest';
import { attribution, replyBody } from './quote';

const WHEN = Date.UTC(2026, 2, 4, 14, 12);

describe('attribution', () => {
  it('names who wrote and when', () => {
    const line = attribution('Dana Wu', WHEN, 'en-GB');
    expect(line).toContain('Dana Wu');
    expect(line).toContain('2026');
    expect(line).toMatch(/wrote:$/);
  });
});

describe('replyBody', () => {
  const body = replyBody('<p>-- </p>', 'Dana Wu', WHEN, '<p>The original.</p>', 'en-GB');

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
