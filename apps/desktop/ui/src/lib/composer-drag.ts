export type ComposerDrag = 'field' | 'file' | 'through';

/**
 * What a drag over the composer means before the drop lands.
 *
 * `field` is text meant for To, Cc, Bcc, or Subject. `through` is a selection
 * moving inside the body. Everything else — including WKWebView file drags
 * that only declare `text/uri-list`, or declare nothing at all — is `file`.
 */
export function classifyComposerDrag(
  types: readonly string[],
  overTextField: boolean,
): ComposerDrag {
  const list = Array.from(types);

  if (list.includes('Files')) return 'file';

  const hasPlain = list.includes('text/plain');
  const hasHtml = list.includes('text/html');
  if (!overTextField && (hasPlain || hasHtml)) return 'through';

  if (overTextField && list.some((t) => t.startsWith('text/'))) return 'field';

  return 'file';
}
