import { describe, expect, it } from 'vitest';
import { classifyLink } from './links';

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
