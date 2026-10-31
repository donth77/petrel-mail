import { describe, expect, it } from 'vitest';
import { classifyComposerDrag } from './composer-drag';

describe('classifyComposerDrag', () => {
  it('takes a uri-list dropped on the reply body as a file', () => {
    // WKWebView often omits "Files" while the pointer is over the quoted
    // body. That is still a file drop. The editor, which the pointer is
    // already over, reads it; a cover on top of the body is handed nothing.
    expect(classifyComposerDrag(['text/uri-list'], false)).toBe('file');
  });

  it('leaves a uri-list dropped on a text field to that field', () => {
    expect(classifyComposerDrag(['text/uri-list'], true)).toBe('field');
  });

  it('takes a file dragged over a text field as a file', () => {
    expect(classifyComposerDrag(['Files', 'text/plain'], true)).toBe('file');
  });

  it('lets a selection move inside the body', () => {
    expect(classifyComposerDrag(['text/plain', 'text/html'], false)).toBe('through');
  });

  it('takes an undescribed drag over the body as a file', () => {
    expect(classifyComposerDrag([], false)).toBe('file');
  });

  it('leaves plain text dropped on a field in that field', () => {
    expect(classifyComposerDrag(['text/plain'], true)).toBe('field');
  });

  it('takes a file the browser named as Files, including over the body', () => {
    expect(classifyComposerDrag(['Files'], false)).toBe('file');
  });

  it('does not cover a body selection that also carries a uri-list as text', () => {
    // A dragged link and an in-editor selection both name text/plain. Treating
    // every uri-list as a file would cover the selection. A file the engine
    // forgot to label as Files still attaches: the editor, which the drag was
    // already over, is the one call that returns the file.
    expect(classifyComposerDrag(['text/uri-list', 'text/plain', 'text/html'], false)).toBe('through');
  });
});
