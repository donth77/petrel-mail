import { describe, expect, it } from 'vitest';
import { plainTextFromDoc, type DocNode } from './plain-text';

const p = (...content: DocNode[]): DocNode => ({ type: 'paragraph', content });
const t = (text: string, marks?: DocNode['marks']): DocNode => ({ type: 'text', text, marks });
const doc = (...content: DocNode[]): DocNode => ({ type: 'doc', content });
const link = (href: string) => [{ type: 'link', attrs: { href } }];

describe('plainTextFromDoc', () => {
  it('keeps paragraphs apart', () => {
    expect(plainTextFromDoc(doc(p(t('One.')), p(t('Two.'))))).toBe('One.\n\nTwo.');
  });

  it('writes a link as text and address', () => {
    expect(plainTextFromDoc(doc(p(t('the docs', link('https://x.example/a'))))))
      .toBe('the docs <https://x.example/a>');
  });

  /* Otherwise the reader gets the same URL twice in a row, which reads as a
     mistake rather than a link. */
  it('does not repeat a link whose text is already the address', () => {
    const url = 'https://x.example/a';
    expect(plainTextFromDoc(doc(p(t(url, link(url)))))).toBe(url);
  });

  it('drops emphasis rather than transliterating it', () => {
    const marked = t('important', [{ type: 'bold' }, { type: 'italic' }]);
    expect(plainTextFromDoc(doc(p(marked)))).toBe('important');
  });

  it('quotes with > on every line, including the blank ones', () => {
    const quoted = {
      type: 'blockquote',
      content: [p(t('First.')), p(t('Second.'))],
    };
    expect(plainTextFromDoc(doc(quoted))).toBe('> First.\n>\n> Second.');
  });

  it('marks bullets and numbers the ordered list', () => {
    const bullets = { type: 'bulletList', content: [
      { type: 'listItem', content: [p(t('one'))] },
      { type: 'listItem', content: [p(t('two'))] },
    ]};
    expect(plainTextFromDoc(doc(bullets))).toBe('- one\n- two');

    const numbers = { type: 'orderedList', attrs: { start: 1 }, content: [
      { type: 'listItem', content: [p(t('first'))] },
      { type: 'listItem', content: [p(t('second'))] },
    ]};
    expect(plainTextFromDoc(doc(numbers))).toBe('1. first\n2. second');
  });

  it('honours a hard break inside a paragraph', () => {
    expect(plainTextFromDoc(doc(p(t('one'), { type: 'hardBreak' }, t('two')))))
      .toBe('one\ntwo');
  });

  /* The empty paragraph everyone leaves at the bottom of an editor. */
  it('does not carry trailing blank paragraphs into the message', () => {
    expect(plainTextFromDoc(doc(p(t('Done.')), p(), p()))).toBe('Done.');
  });

  it('survives a node it has never seen without losing the words', () => {
    const odd = { type: 'somethingNew', content: [t('still here')] };
    expect(plainTextFromDoc(doc(odd))).toContain('still here');
  });

  it('is empty for an empty document', () => {
    expect(plainTextFromDoc(doc())).toBe('');
    expect(plainTextFromDoc(null)).toBe('');
  });
});
