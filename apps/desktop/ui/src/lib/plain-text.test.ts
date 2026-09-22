import { describe, expect, it } from 'vitest';
import { plainTextFromDoc, type DocNode } from './plain-text';

const p = (...content: DocNode[]): DocNode => ({ type: 'paragraph', content });
const t = (text: string, marks?: DocNode['marks']): DocNode => ({ type: 'text', text, marks });
const doc = (...content: DocNode[]): DocNode => ({ type: 'doc', content });
const link = (href: string) => [{ type: 'link', attrs: { href } }];

describe('plainTextFromDoc', () => {
  /* A paragraph is a line, as the composer draws it: Enter goes to the next
     one with no gap. */
  it('puts each paragraph on a line of its own', () => {
    expect(plainTextFromDoc(doc(p(t('One.')), p(t('Two.'))))).toBe('One.\nTwo.');
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

  it('quotes with > on every line', () => {
    const quoted = {
      type: 'blockquote',
      content: [p(t('First.')), p(t('Second.'))],
    };
    expect(plainTextFromDoc(doc(quoted))).toBe('> First.\n> Second.');
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

  /* A blank line between sentences is a paragraph mark in CJK, not leftover
     chrome. Collapsing three-or-more newlines used to fold every run down to
     one blank, so none, one and three typed blank lines all read the same.

     Counted, not merely preserved: every empty paragraph the author typed is
     one blank line, exactly as the composer shows it, and no more. */
  it('keeps every blank line the author typed, and adds none', () => {
    const blanks = (out: string) => out.split('\n').filter((l) => l === '').length;
    expect(blanks(plainTextFromDoc(doc(p(t('One.')), p(t('Two.')))))).toBe(0);
    expect(blanks(plainTextFromDoc(doc(p(t('One.')), p(), p(t('Two.')))))).toBe(1);
    expect(blanks(plainTextFromDoc(doc(p(t('One.')), p(), p(), p(t('Two.')))))).toBe(2);
    expect(blanks(plainTextFromDoc(doc(p(t('One.')), p(), p(), p(), p(t('Two.')))))).toBe(3);
    expect(plainTextFromDoc(doc(p(t('One.')), p(), p(t('Two.'))))).toBe('One.\n\nTwo.');
  });

  /* The gap inside a quote is the original author's too. */
  it('counts blank lines inside a quote as well', () => {
    const quoted = plainTextFromDoc(
      doc({ type: 'blockquote', content: [p(t('One.')), p(), p(t('Two.'))] }),
    );
    expect(quoted).toBe('> One.\n>\n> Two.');
  });

  /* Every client folds a signature on `-- ` exactly, trailing space and all.
     The editor keeps that space as a non-breaking one; the text half sends an
     ordinary one, and trimming trailing spaces must not take it. */
  it('sends the signature separator with its space', () => {
    expect(plainTextFromDoc(doc(p(t('Hi')), p(t('--\u00a0')), p(t('You'))))).toBe('Hi\n-- \nYou');
    expect(plainTextFromDoc(doc(p(t('Hi')), p(t('-- ')), p(t('You'))))).toBe('Hi\n-- \nYou');
  });

  it('survives a node it has never seen without losing the words', () => {
    const odd = { type: 'somethingNew', content: [t('still here')] };
    expect(plainTextFromDoc(doc(odd))).toContain('still here');
  });

  it('is empty for an empty document', () => {
    expect(plainTextFromDoc(doc())).toBe('');
    expect(plainTextFromDoc(null)).toBe('');
  });

  it('a pasted image leaves a placeholder in the text half', () => {
    const doc = {
      type: 'doc',
      content: [
        { type: 'paragraph', content: [{ type: 'text', text: 'Look:' }] },
        { type: 'image', attrs: { src: 'data:image/png;base64,xxxx', alt: null } },
        { type: 'paragraph', content: [{ type: 'text', text: 'seen?' }] },
      ],
    };
    // The convention incoming mail uses, so a text-only client reads a
    // message, not a hole.
    expect(plainTextFromDoc(doc)).toBe('Look:\n[image]\nseen?');
  });
});
