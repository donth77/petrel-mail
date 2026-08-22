/**
 * The plain-text half of a message, generated from the rich-text document.
 *
 * Every message goes out as `multipart/alternative`: the HTML for clients that
 * render it, and this for the ones that do not — and for the people who prefer
 * it, who are not a rounding error on a mailing list.
 *
 * Built from the editor's document tree rather than from its HTML output. The
 * tree is what we control: the schema is closed, so every node here is one this
 * app can produce, and there is no HTML parser to disagree with. Converting the
 * rendered HTML back to text would mean re-deriving structure that was already
 * known, and getting it subtly wrong on the cases that matter — a blockquote
 * inside a list, a link whose text is itself a URL.
 *
 * Emphasis is dropped rather than transliterated. `*bold*` is a convention from
 * a different medium, and a reader who has only the text part is better served
 * by clean prose than by punctuation standing in for formatting they cannot
 * see. Links and quoting are kept, because those carry information the words
 * alone do not: where a link goes, and who said what.
 */

/** A node of the editor's document, as it serialises. */
export type DocNode = {
  type?: string;
  text?: string;
  content?: DocNode[];
  marks?: { type: string; attrs?: Record<string, unknown> }[];
  attrs?: Record<string, unknown>;
};

/** One run of text, with its link resolved if it has one. */
function runToText(node: DocNode): string {
  const text = node.text ?? '';
  const link = node.marks?.find((m) => m.type === 'link');
  const href = typeof link?.attrs?.href === 'string' ? link.attrs.href : null;
  if (!href) return text;
  // A link whose text is already the address reads as a stutter written out
  // twice — "https://x.example <https://x.example>" tells the reader nothing
  // the first copy did not.
  if (text.trim() === href.trim()) return href;
  return `${text} <${href}>`;
}

/** Renders the children of a block into one line, honouring hard breaks. */
function inline(nodes: DocNode[] | undefined): string {
  if (!nodes) return '';
  return nodes
    .map((n) => (n.type === 'hardBreak' ? '\n' : n.type === 'text' ? runToText(n) : block(n)))
    .join('');
}

/** Prefixes every line, including the empty ones inside a quote. */
function prefixLines(text: string, prefix: string): string {
  return text
    .split('\n')
    .map((line) => (line ? `${prefix}${line}` : prefix.trimEnd()))
    .join('\n');
}

function block(node: DocNode): string {
  switch (node.type) {
    case 'doc':
      return (node.content ?? []).map(block).filter((s) => s !== null).join('\n\n');

    case 'paragraph':
      return inline(node.content);

    case 'heading':
      // Headings are off in the composer's schema, but a draft written before
      // that or pasted in can still carry one, and dropping the text would lose
      // the sentence rather than the styling.
      return inline(node.content);

    case 'blockquote':
      return prefixLines((node.content ?? []).map(block).join('\n\n'), '> ');

    case 'bulletList':
      return (node.content ?? []).map((li) => `- ${listItem(li)}`).join('\n');

    case 'orderedList': {
      const start = typeof node.attrs?.start === 'number' ? node.attrs.start : 1;
      return (node.content ?? []).map((li, i) => `${start + i}. ${listItem(li)}`).join('\n');
    }

    case 'codeBlock':
      return inline(node.content);

    case 'horizontalRule':
      return '---';

    case 'hardBreak':
      return '\n';

    case 'text':
      return runToText(node);

    default:
      // An unknown node still has words in it somewhere. Losing formatting is
      // recoverable; losing a sentence is not.
      return node.content ? (node.content ?? []).map(block).join('') : (node.text ?? '');
  }
}

/** A list item's text, with continuation lines indented under the marker. */
function listItem(node: DocNode): string {
  const body = (node.content ?? []).map(block).join('\n');
  const [first, ...rest] = body.split('\n');
  return [first, ...rest.map((line) => (line ? `  ${line}` : line))].join('\n');
}

/**
 * The whole document as text.
 *
 * Trailing whitespace is trimmed per line and the result ends without a run of
 * blank lines, because an empty paragraph at the end of an editor is a thing
 * people leave behind constantly and it should not travel.
 */
export function plainTextFromDoc(doc: DocNode | null | undefined): string {
  if (!doc) return '';
  return block(doc)
    .split('\n')
    .map((line) => line.replace(/[ \t]+$/, ''))
    .join('\n')
    .replace(/\n{3,}/g, '\n\n')
    .trim();
}
