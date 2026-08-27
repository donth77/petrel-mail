import { describe, expect, it } from 'vitest';
import { classifyLink, homographRisk } from './links';

describe('classifyLink', () => {
  it('sends web links to the browser', () => {
    expect(classifyLink('https://example.com/a?b=1')).toEqual({
      kind: 'web',
      url: 'https://example.com/a?b=1',
    });
    expect(classifyLink('http://old.example/x.html').kind).toBe('web');
  });

  it('keeps mail links in Petrel, with just the address', () => {
    expect(classifyLink('mailto:sam@example.com')).toEqual({ kind: 'mail', addr: 'sam@example.com' });
    expect(classifyLink('mailto:sam@example.com?subject=Hi%20there')).toEqual({
      kind: 'mail',
      addr: 'sam@example.com',
    });
    expect(classifyLink('mailto:a%2Bb@example.com')).toEqual({ kind: 'mail', addr: 'a+b@example.com' });
  });

  it('opens nothing else, whoever sent it', () => {
    for (const href of [
      'javascript:alert(1)',
      'file:///etc/passwd',
      'data:text/html,<script>x</script>',
      'petrel-msg://localhost/message/1',
      'vscode://file/Users/me/.ssh/id_rsa',
      'mailto:',
      '',
      'about:blank',
    ]) {
      expect(classifyLink(href), href).toEqual({ kind: 'blocked' });
    }
  });

  it('is not fooled by case or padding', () => {
    expect(classifyLink('  HTTPS://Example.com/  ').kind).toBe('web');
    expect(classifyLink('JavaScript:alert(1)').kind).toBe('blocked');
  });
});

describe('homographRisk', () => {
  it('says nothing about ordinary links', () => {
    expect(homographRisk('https://apple.com/store')).toBeNull();
    expect(homographRisk('http://localhost:5199/x')).toBeNull();
    expect(homographRisk('not a url')).toBeNull();
  });

  it('catches a Latin name spelled with another alphabet', () => {
    // "аpple.com" — the first letter is Cyrillic а, and the rest is Latin.
    const risk = homographRisk('https://аpple.com/login');
    expect(risk).not.toBeNull();
    expect(risk!.reason).toBe('mixed-script');
    expect(risk!.asPunycode).toContain('xn--');
    expect(risk!.asTyped).toBe('аpple.com');
  });

  it('catches a name built from lookalikes even without an ASCII tld', () => {
    // With a Latin .com the mixing itself gives it away, so the interesting
    // case is a name where every part is Cyrillic — nothing to mix with,
    // and every letter still chosen to pass for a Latin one.
    const risk = homographRisk('https://раураӏ.рф');
    expect(risk?.reason).toBe('latin-lookalike');
    // And the everyday version, where the ASCII tld makes it mixed.
    expect(homographRisk('https://раураӏ.com')?.reason).toBe('mixed-script');
  });

  it('leaves honest international domains alone', () => {
    // A Japanese domain is somebody's real address, not a disguise.
    expect(homographRisk('https://日本語.jp')).toBeNull();
    // As is a German one with an umlaut.
    expect(homographRisk('https://münchen.de')).toBeNull();
  });
});
