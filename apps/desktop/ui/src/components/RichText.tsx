import { useEffect, useRef, useState } from 'react';
import {
  Select, SelectItem, SelectPopover, SelectProvider,
} from '@ariakit/react';
import { EditorContent, generateJSON, useEditor, type Editor, type Extensions } from '@tiptap/react';
import { Selection, TextSelection } from '@tiptap/pm/state';
import StarterKit from '@tiptap/starter-kit';
import { Blockquote } from '@tiptap/extension-blockquote';
import { Image } from '@tiptap/extension-image';
import { TextStyle } from '@tiptap/extension-text-style';
import { FontFamily } from '@tiptap/extension-text-style/font-family';
import { FontSize } from '@tiptap/extension-text-style/font-size';
import {
  Bold, ChevronDown, Code, Italic, Link2, List, ListOrdered, Quote,
  Strikethrough, Type, Underline, type LucideIcon,
} from 'lucide-react';
import { plainTextFromDoc, type DocNode } from '../lib/plain-text';
import { EMBED_CAP, asDataUrl, pastedImages } from '../lib/paste-image';
import { Icon } from './Icon';
import { t, type StringId } from '../lib/strings';
import { Tip } from './Tip';
import { nextTool, typedIntoBody } from '../lib/toolbar';
import { filesFromDataTransfer } from '../lib/dropped-files';

/** The typefaces on offer.
 *
 * Four, not twenty. A font the recipient does not have installed is not a
 * choice they see — it is a fallback nobody picked — and every stack here
 * resolves on any machine that will open the message. The empty value means no
 * font-family at all, so ordinary text carries no markup.
 */
const FONTS: { label: StringId; stack: string }[] = [
  { label: 'font-default', stack: '' },
  { label: 'font-sans', stack: 'Arial, Helvetica, sans-serif' },
  { label: 'font-serif', stack: 'Georgia, "Times New Roman", serif' },
  { label: 'font-mono', stack: 'ui-monospace, "SFMono-Regular", Menlo, monospace' },
];

/** Named sizes rather than a px field: nobody composing an email thinks in
 *  pixels, and the four names cover what the choice is actually for. */
const SIZES: { label: StringId; css: string }[] = [
  { label: 'size-small', css: '12px' },
  { label: 'size-normal', css: '' },
  { label: 'size-large', css: '18px' },
  { label: 'size-huge', css: '24px' },
];

/** Which entry the caret is currently sitting in, for the select's value. */
function currentFont(editor: Editor): string {
  const active = editor.getAttributes('textStyle').fontFamily as string | undefined;
  return FONTS.find((f) => f.stack && f.stack === active)?.stack ?? '';
}

function currentSize(editor: Editor): string {
  const active = editor.getAttributes('textStyle').fontSize as string | undefined;
  return SIZES.find((z) => z.css && z.css === active)?.css ?? '';
}

/**
 * A blockquote that keeps `type="cite"`.
 *
 * The stock node drops unknown attributes, and this is the one attribute that
 * matters: Apple Mail, Thunderbird and Outlook decide what to fold behind
 * "show quoted text" by looking for it. Without it every reply in a long
 * thread carries an unfoldable copy of the whole history — which is precisely
 * what quoting was added to avoid.
 */
const CiteBlockquote = Blockquote.extend({
  addAttributes() {
    return {
      ...this.parent?.(),
      type: {
        default: null,
        parseHTML: (el) => el.getAttribute('type'),
        renderHTML: (attrs) => (attrs.type ? { type: attrs.type as string } : {}),
      },
    };
  },
});

/** The composer's schema: what the editor edits, and what `plainTextOf` reads
 *  a body with, so the two cannot come to disagree. */
export function composerExtensions(): Extensions {
  return [
    StarterKit.configure({
      // Not a document, and not a place to paste a file.
      heading: false,
      // Replaced below, to keep the cite attribute quoting depends on.
      blockquote: false,
      codeBlock: false,
      horizontalRule: false,
      link: {
        // In an editor a click means "put the caret here", not "leave".
        openOnClick: false,
        autolink: true,
        // A pasted URL becomes a link; a pasted anything-else does not.
        protocols: ['http', 'https', 'mailto'],
      },
    }),
    // Font family and size ride on a text-style mark, which renders as an
    // inline style — the only form of styling mail clients agree on. A
    // stylesheet in the head is stripped by most webmail, and a class name
    // means nothing at the other end.
    CiteBlockquote,
    // Pasted pictures. Base64 stays allowed because a draft *is* a data:
    // URI until send, when each becomes a MIME part of its own.
    Image.configure({ allowBase64: true }),
    TextStyle,
    FontFamily,
    FontSize,
  ];
}

/**
 * The plain-text half of a body the editor has not touched yet.
 *
 * The editor only reports its document when something changes it, and it does
 * not count its own starting content as a change. A reply got its text half by
 * accident — putting the caret in the body appends a line after the quote —
 * but a forward opens with the caret in To, so one sent without an edit went
 * out with a text half that was the signature alone: nothing at all to anyone
 * reading plain text, and all Petrel's own Sent copy was ever indexed by. Read
 * here from the same document the editor makes of the same HTML.
 */
export function plainTextOf(html: string): string {
  return plainTextFromDoc(generateJSON(html, composerExtensions()) as DocNode);
}

/** What the composer around the editor may ask of it. */
export type RichTextHandle = {
  /** Focus with the caret at the top of the body, where a message is
   *  written: Tab from Subject arrives here. */
  focusStart: () => void;
  /** Focus where the caret already was. */
  focus: () => void;
};

/** The toolbar's controls, in order: the two pickers, then the buttons. */
const TOOL_COUNT = 11;

type Props = {
  /** The body as HTML. Read on mount and when the draft is swapped, not on
   *  every keystroke — the editor owns its own content while it is open. */
  html: string;
  /** Both halves, every change: the HTML that is sent and the document the
   *  plain-text alternative is generated from. */
  onChange: (html: string, doc: DocNode) => void;
  onKeyDown?: (e: React.KeyboardEvent) => void;
  autoFocus?: boolean;
  /** Something worth saying that is not worth a dialog — a refused paste. */
  onNotice?: (text: string) => void;
  /** Files dragged onto the body become attachments, not inline images. */
  onDropFiles?: (files: readonly File[]) => void;
  /** Filled in once the editor exists. */
  handle?: { current: RichTextHandle | null };
};

/**
 * The message body, as rich text.
 *
 * The schema is closed on purpose, and smaller than the editor could offer.
 * Mail is read in clients whose CSS support ranges from good to 1997, so
 * everything here has to survive being rendered by something that has never
 * heard of it: bold, italic, underline, strike, links, lists, quotes and
 * inline code. Headings are off — an email is not a document, and a heading in
 * a two-paragraph message is a font size looking for a purpose. Code blocks are
 * off for the same reason inline code is on: people quote a line, not a file.
 *
 * Markdown-ish input rules come with the kit and are the point of using it:
 * `**bold**`, `- ` for a list, `> ` for a quote. Someone who types markdown out
 * of habit gets what they meant rather than the punctuation.
 */
export function RichText({ html, onChange, onKeyDown, autoFocus, onNotice, onDropFiles, handle }: Props) {
  // The link card: its own text and address, so a link can be labelled rather
  // than only wrapped around whatever happened to be selected.
  const [link, setLink] = useState<
    { text: string; href: string; top: number; left: number; existing?: boolean } | null
  >(null);
  const linkText = useRef<HTMLInputElement>(null);
  const linkCard = useRef<HTMLDivElement>(null);
  const shell = useRef<HTMLDivElement>(null);
  // An address the card was dismissed for, so putting it away does not have it
  // spring straight back while the caret is still sitting in the same link.
  const dismissed = useRef<string | null>(null);
  // The address the menu is currently showing, read by the outside-click
  // listener. A ref rather than a dependency: the listener would otherwise be
  // torn down and rebuilt on every keystroke in the address field.
  const openHref = useRef<string>('');
  // The toolbar's one tab stop: the control focus last rested on, so Shift+Tab
  // back from the body returns to it.
  const [tool, setTool] = useState(0);
  const tools = useRef<HTMLDivElement>(null);
  const dropRef = useRef(onDropFiles);
  dropRef.current = onDropFiles;

  const editor = useEditor({
    extensions: composerExtensions(),
    content: html || '',
    editorProps: {
      attributes: {
        class: 'rich-body',
        'aria-label': t('compose-body'),
      },
      // A pasted screenshot lands in the body, where it was aimed. Only a
      // clipboard that is purely images counts (the policy's call, not
      // this one's); text pastes fall through to the editor untouched.
      handlePaste: (view, event) => {
        const images = pastedImages(event.clipboardData);
        if (images.length === 0) return false;
        for (const file of images) {
          if (file.size > EMBED_CAP) {
            onNotice?.(t('compose-img-too-big'));
            continue;
          }
          void asDataUrl(file).then((src) => {
            // Fresh state on arrival: reading the file is asynchronous and
            // the document may have moved under it.
            const { image, paragraph } = view.state.schema.nodes;
            let tr = view.state.tr.replaceSelectionWith(image.create({ src }));
            // The caret goes after the picture, not on it. Left selected,
            // the next thing typed would replace what was just pasted — and
            // when the picture is the last thing in the message there is no
            // text position to move to until a paragraph is put there.
            const at = tr.selection.to;
            if (!tr.doc.resolve(at).nodeAfter?.isTextblock) {
              tr = tr.insert(at, paragraph.create());
            }
            tr = tr.setSelection(TextSelection.near(tr.doc.resolve(at), 1));
            view.dispatch(tr.scrollIntoView());
          });
        }
        return true;
      },
      handleDrop: (_view, event) => {
        const files = filesFromDataTransfer(event.dataTransfer);
        if (files.length === 0) return false;
        event.preventDefault();
        // This element is the one the drag was over, so it is the read that
        // WKWebView will answer. Stopping it keeps the composer's own drop
        // from asking a second time and getting nothing.
        event.stopPropagation();
        dropRef.current?.(files);
        return true;
      },
    },
    onUpdate: ({ editor }) => onChange(editor.getHTML(), editor.getJSON() as DocNode),
    // The caret landing in a link opens the link menu itself, prefilled. There
    // is no intermediate bubble asking whether you meant it: a step whose only
    // outcome is the next step is a step to remove.
    onSelectionUpdate: ({ editor }) => caretMoved(editor),
    // A click that lands in a link changes the selection but not the document,
    // and focus can arrive without either.
    onFocus: ({ editor }) => caretMoved(editor),
    // The editor is mounted inside a React tree that re-renders often; without
    // this Tiptap warns and can flush into a render pass.
    immediatelyRender: false,
  });

  // At the top, where a reply is written above the quote and a new message
  // above its signature. The end put the caret below the signature, and in a
  // reply at the foot of the quoted original, inside it.
  //
  // The caret first, as a transaction of its own, and then focus. As the one
  // command `focus('start')`, a WebKit that names itself Safari (WebKitGTK on
  // Linux; not the Mac's own web view) focused the editor from inside it; the
  // editor settled there — a quote at the end gets a line after it — and the
  // command's transaction no longer fitted the document: "Applying a
  // mismatched transaction", thrown from this effect, took the whole window
  // down on every reply. A frame late too, so nothing thrown in here can do
  // that again.
  useEffect(() => {
    if (!autoFocus || !editor) return;
    const h = requestAnimationFrame(() => {
      if (editor.isDestroyed) return;
      editor.commands.command(({ tr }) => {
        tr.setSelection(Selection.atStart(tr.doc));
        return true;
      });
      editor.commands.focus();
    });
    return () => cancelAnimationFrame(h);
  }, [autoFocus, editor]);

  useEffect(() => {
    if (!handle) return;
    handle.current = editor
      ? {
          // The caret first, as a transaction of its own, as the autofocus
          // above does it; then the view itself, at once. The editor's focus
          // command waits a frame, and a typist does not: the first letter
          // after Tab went into Subject.
          focusStart: () => {
            if (editor.isDestroyed) return;
            editor.commands.command(({ tr }) => {
              tr.setSelection(Selection.atStart(tr.doc));
              return true;
            });
            editor.view.focus();
          },
          focus: () => {
            if (!editor.isDestroyed) editor.view.focus();
          },
        }
      : null;
    return () => {
      handle.current = null;
    };
  }, [editor, handle]);

  // Whether the card is up, not the card itself: focus belongs on the first
  // field when it opens, and must not be yanked back there on every keystroke.
  const linkOpen = link !== null;
  openHref.current = link?.href ?? '';
  useEffect(() => {
    if (linkOpen) linkText.current?.focus();
  }, [linkOpen]);

  // Clicking away puts the card down. Not onBlur, which fires when focus moves
  // between the card's own two fields and would close it mid-edit; and pointer
  // down rather than click, so it goes away as the press lands rather than
  // after it, which is when a menu feels stuck.
  useEffect(() => {
    if (!linkOpen) return;
    const away = (e: PointerEvent) => {
      if (linkCard.current?.contains(e.target as Node)) return;
      dismissed.current = openHref.current || null;
      setLink(null);
    };
    // Deferred a tick: the press that opened the card is still being delivered,
    // and would otherwise close it immediately.
    const h = setTimeout(() => document.addEventListener('pointerdown', away), 0);
    return () => {
      clearTimeout(h);
      document.removeEventListener('pointerdown', away);
    };
  }, [linkOpen]);

  if (!editor) return <div className="rich-shell" />;

  /** A character typed on one of the toolbar's closed menus, written where
   *  the caret was in the message, which takes focus back with it, so the
   *  rest of the sentence follows by itself. As text, never as markup. */
  const typeIntoBody = (text: string) => {
    editor.chain().focus().insertContent({ type: 'text', text }).run();
  };

  /** Follows the caret in and out of links, opening and closing the menu. */
  const caretMoved = (ed: Editor) => {
    const found = linkUnderCaret(ed, shell.current);
    if (!found) {
      // Out of every link: forget any dismissal, and put away a menu that was
      // only up because of the link just left.
      dismissed.current = null;
      setLink((cur) => (cur?.existing ? null : cur));
      return;
    }
    if (dismissed.current === found.href) return;
    setLink((cur) => {
      // Already editing this link — do not reset the fields under the typist.
      if (cur && cur.href === found.href) return cur;
      return {
        text: linkTextAt(ed),
        href: found.href,
        top: found.top,
        left: found.left,
        existing: true,
      };
    });
  };

  /** Opens the card near the text it is about to link, not on top of it. */
  const openLink = (href = '', existing = false) => {
    const { from, to } = editor.state.selection;
    const box = shell.current?.getBoundingClientRect();
    // Rough, and only needs to be: the card is measured once it exists, and
    // these decide which side of the line it opens on.
    const CARD = { w: 330, h: 96, gap: 8, edge: 8 };
    let top = CARD.edge;
    let left = CARD.edge;
    if (box) {
      const caret = editor.view.coordsAtPos(to);
      // Below the line by preference — reading order, and it leaves the text
      // being linked in view above it.
      top = caret.bottom - box.top + CARD.gap;
      left = caret.left - box.left;
      // Flip above when there is no room below, rather than hanging off the
      // bottom of a composer that does not scroll for it.
      if (top + CARD.h > box.height - CARD.edge) {
        top = caret.top - box.top - CARD.h - CARD.gap;
      }
      top = Math.max(CARD.edge, top);
      left = Math.max(CARD.edge, Math.min(left, box.width - CARD.w - CARD.edge));
    }
    setLink({ text: editor.state.doc.textBetween(from, to, ' '), href, top, left, existing });
  };

  const apply = () => {
    if (!link) return;
    const href = link.href.trim();
    setLink(null);
    if (!href) {
      editor.commands.focus();
      return;
    }
    // A bare domain is what people type; without a scheme the link resolves
    // against the message frame and goes nowhere.
    const url = /^[a-z][a-z0-9+.-]*:/i.test(href) ? href : `https://${href}`;
    const label = link.text.trim();
    const { from, to } = editor.state.selection;
    const chain = editor.chain().focus();
    if (label && label !== editor.state.doc.textBetween(from, to, ' ')) {
      // The text was written or changed in the card, so it replaces whatever
      // was selected — that is what typing in a field called Text means.
      chain.insertContent({ type: 'text', text: label, marks: [{ type: 'link', attrs: { href: url } }] });
    } else if (from === to) {
      chain.insertContent({ type: 'text', text: label || href, marks: [{ type: 'link', attrs: { href: url } }] });
    } else {
      chain.setLink({ href: url });
    }
    chain.run();
  };

  return (
    <div className="rich-shell" ref={shell} onKeyDown={onKeyDown}>
      <div
        className="rich-tools"
        role="toolbar"
        aria-label={t('compose-formatting')}
        ref={tools}
        onKeyDown={(e) => {
          // The controls only: a picker's open list is rendered elsewhere but
          // its keys bubble here through React, and they are its own.
          const at = (e.target as HTMLElement).closest?.('[data-tool]');
          if (!at || !tools.current?.contains(at)) return;
          const next = nextTool(tool, e.key, TOOL_COUNT);
          if (next === null) return;
          e.preventDefault();
          setTool(next);
          tools.current.querySelector<HTMLElement>(`[data-tool="${next}"]`)?.focus();
        }}
      >
        {/* Web-safe stacks only. A typeface the recipient does not have is not
            a choice, it is a fallback nobody picked — so the list is the four
            that resolve everywhere rather than the twenty that look richer. */}
        <Picker
          tool={0}
          current={tool}
          onFocusTool={setTool}
          onType={typeIntoBody}
          label={t('format-font')}
          value={currentFont(editor)}
          options={FONTS.map((f) => ({
            value: f.stack,
            label: t(f.label),
            // Each name drawn in its own face: a font list that does not show
            // the fonts is a list of words to guess between.
            style: { fontFamily: f.stack || undefined },
          }))}
          onPick={(value) =>
            value
              ? editor.chain().focus().setFontFamily(value).run()
              : editor.chain().focus().unsetFontFamily().run()
          }
        />
        <Picker
          tool={1}
          current={tool}
          onFocusTool={setTool}
          onType={typeIntoBody}
          label={t('format-size')}
          narrow
          value={currentSize(editor)}
          options={SIZES.map((z) => ({
            value: z.css,
            label: t(z.label),
            // Shown at the size it applies, which is the whole information.
            style: { fontSize: z.css || undefined },
          }))}
          onPick={(value) =>
            // Normal removes the style rather than writing the default size,
            // so ordinary text carries no markup at all.
            value
              ? editor.chain().focus().setFontSize(value).run()
              : editor.chain().focus().unsetFontSize().run()
          }
        />
        <span className="rich-sep" aria-hidden="true" />
        <Mark tool={2} current={tool} onFocusTool={setTool} editor={editor} name="bold" icon={Bold} label={t('format-bold')} keys="⌘B" />
        <Mark tool={3} current={tool} onFocusTool={setTool} editor={editor} name="italic" icon={Italic} label={t('format-italic')} keys="⌘I" />
        <Mark
          tool={4}
          current={tool} onFocusTool={setTool}
          editor={editor}
          name="underline"
          icon={Underline}
          label={t('format-underline')}
          keys="⌘U"
        />
        <Mark tool={5} current={tool} onFocusTool={setTool} editor={editor} name="strike" icon={Strikethrough} label={t('format-strike')} />
        <Mark tool={6} current={tool} onFocusTool={setTool} editor={editor} name="code" icon={Code} label={t('format-code')} />
        <span className="rich-sep" aria-hidden="true" />
        <Node
          tool={7}
          current={tool} onFocusTool={setTool}
          editor={editor}
          name="bulletList"
          icon={List}
          label={t('format-bullets')}
          run={() => editor.chain().focus().toggleBulletList().run()}
        />
        <Node
          tool={8}
          current={tool} onFocusTool={setTool}
          editor={editor}
          name="orderedList"
          icon={ListOrdered}
          label={t('format-numbers')}
          run={() => editor.chain().focus().toggleOrderedList().run()}
        />
        <Node
          tool={9}
          current={tool} onFocusTool={setTool}
          editor={editor}
          name="blockquote"
          icon={Quote}
          label={t('format-quote')}
          run={() => editor.chain().focus().toggleBlockquote().run()}
        />
        <Node
          tool={10}
          current={tool} onFocusTool={setTool}
          editor={editor}
          name="link"
          icon={Link2}
          label={editor.isActive('link') ? t('format-unlink') : t('format-link')}
          run={() => {
            // In a link the menu is already open, or was dismissed — either
            // way this brings it back rather than destroying the link. A
            // control called "add a link" quietly deleting one is the wrong
            // kind of surprise; Remove lives in the menu.
            if (editor.isActive('link')) {
              const href = editor.getAttributes('link').href as string | undefined;
              dismissed.current = null;
              editor.chain().focus().extendMarkRange('link').run();
              openLink(href ?? '', true);
              return;
            }
            openLink();
          }}
        />
      </div>

      {link && (
        // Floating, not inserted. A row between the toolbar and the body pushed
        // the message down every time this opened, which moved the words the
        // link was being added to.
        <div
          className="rich-link-card"
          ref={linkCard}
          style={{ insetBlockStart: link.top, insetInlineStart: link.left }}
        >
          <label className="rich-link-row">
            <Icon icon={Type} size={13} />
            <input
              ref={linkText}
              className="compose-input"
              placeholder={t('format-link-text')}
              aria-label={t('format-link-text')}
              value={link.text}
              autoComplete="off"
              onChange={(e) => setLink({ ...link, text: e.target.value })}
              onKeyDown={(e) => e.stopPropagation()}
            />
          </label>
          {/* The field and Apply are siblings, and only the field is boxed —
              Apply acts on both rows, so it sits outside either one. */}
          <div className="rich-link-apply-row">
            <label className="rich-link-row">
              <Icon icon={Link2} size={13} />
              <input
                className="compose-input"
                placeholder={t('format-link-placeholder')}
                aria-label={t('format-link')}
                value={link.href}
                autoComplete="off"
                spellCheck={false}
                onChange={(e) => setLink({ ...link, href: e.target.value })}
                onKeyDown={(e) => {
                  // Kept off the app's single-key shortcuts, like every field.
                  e.stopPropagation();
                  if (e.key === 'Escape') {
                    dismissed.current = link.href || null;
                    setLink(null);
                    editor.commands.focus();
                  }
                  if (e.key === 'Enter') {
                    e.preventDefault();
                    apply();
                  }
                }}
              />
            </label>
            {/* Only when there is a link to remove. Reaching this card means
                deciding what the link should be, and "none" is one of the
                answers — closing it to find the bubble again is a detour. */}
            {editor.isActive('link') && (
              <button
                type="button"
                className="rich-apply danger"
                onMouseDown={(e) => {
                  e.preventDefault();
                  setLink(null);
                  editor.chain().focus().extendMarkRange('link').unsetLink().run();
                }}
              >
                {t('format-link-remove')}
              </button>
            )}
            <button
              type="button"
              className="rich-apply"
              disabled={!link.href.trim()}
              onMouseDown={(e) => {
                e.preventDefault();
                apply();
              }}
            >
              {t('format-link-apply')}
            </button>
          </div>
        </div>
      )}

      <EditorContent editor={editor} className="rich-content" />
    </div>
  );
}

/** The full text of the link the caret is in, not just what is selected.
 *
 * Walks out to both edges of the mark rather than reading the text node under
 * the caret. A link is often several text nodes — the moment part of it is
 * bold, or the caret sits on a boundary, the single-node reading returns a
 * fragment or nothing at all. It returned nothing, which is how the Text field
 * came up empty on a link that plainly had text. */
function linkTextAt(editor: Editor): string {
  const { state } = editor;
  const type = state.schema.marks.link;
  const { $from } = state.selection;
  if (!type) return '';

  const parent = $from.parent;
  let index = $from.index();
  // A caret at the very end of a node reports the index past it.
  if (index >= parent.childCount) index = parent.childCount - 1;
  if (index < 0 || !type.isInSet(parent.child(index).marks)) return '';

  let start = $from.start();
  for (let i = 0; i < index; i += 1) start += parent.child(i).nodeSize;
  let end = start + parent.child(index).nodeSize;

  while (index > 0 && type.isInSet(parent.child(index - 1).marks)) {
    index -= 1;
    start -= parent.child(index).nodeSize;
  }
  let after = $from.index() + 1;
  if (after > parent.childCount) after = parent.childCount;
  while (after < parent.childCount && type.isInSet(parent.child(after).marks)) {
    end += parent.child(after).nodeSize;
    after += 1;
  }
  return state.doc.textBetween(start, end, ' ');
}

/** The link the caret is inside, and where its menu should sit. */
function linkUnderCaret(
  editor: Editor,
  shell: HTMLElement | null,
): { href: string; top: number; left: number } | null {
  if (!editor.isActive('link')) return null;
  const href = editor.getAttributes('link').href as string | undefined;
  if (!href) return null;
  const box = shell?.getBoundingClientRect();
  if (!box) return null;
  const caret = editor.view.coordsAtPos(editor.state.selection.from);
  return {
    href,
    top: Math.max(4, caret.bottom - box.top + 6),
    left: Math.max(8, Math.min(caret.left - box.left, box.width - 260)),
  };
}

/**
 * A dropdown whose items are drawn the way they will apply.
 *
 * Not a native `<select>`, which is what this was first. Styling `<option>` is
 * unreliable across engines and ignored outright by WKWebView, which is the one
 * the desktop app actually runs — so the preview could not have been verified
 * where it matters. This renders its own list and can be seen to work.
 */
/** A control's place in the toolbar's single tab stop. */
type Roving = {
  tool: number;
  current: number;
  onFocusTool: (tool: number) => void;
};

function rove({ tool, current, onFocusTool }: Roving) {
  return {
    'data-tool': tool,
    tabIndex: tool === current ? 0 : -1,
    onFocus: () => onFocusTool(tool),
  };
}

function Picker({
  label, value, options, onPick, onType, narrow, ...roving
}: {
  label: string;
  value: string;
  options: { value: string; label: string; style?: React.CSSProperties }[];
  onPick: (value: string) => void;
  /** A character typed on the closed menu, which belongs in the message:
   *  see `typedIntoBody`. */
  onType: (text: string) => void;
  narrow?: boolean;
} & Roving) {
  const current = options.find((o) => o.value === value) ?? options[0];
  return (
    <SelectProvider
      value={value}
      setValue={(v) => onPick(String(v))}
      placement="bottom-start"
    >
      <Tip label={label} placement="top">
        <Select
          className={narrow ? 'rich-select narrow' : 'rich-select'}
          aria-label={label}
          {...rove(roving)}
          // In the capture phase, which is where the menu's own type-to-select
          // reads the key, and which it skips once the key is cancelled.
          onKeyDownCapture={(e) => {
            const open = e.currentTarget.getAttribute('aria-expanded') === 'true';
            const key = {
              key: e.key,
              metaKey: e.metaKey,
              ctrlKey: e.ctrlKey,
              altKey: e.altKey,
              isComposing: e.nativeEvent.isComposing,
            };
            if (!typedIntoBody(key, open)) return;
            e.preventDefault();
            onType(e.key);
          }}
        >
          <span className="clip">{current?.label}</span>
          <ChevronDown size={12} aria-hidden="true" />
        </Select>
      </Tip>
      <SelectPopover portal gutter={4} className="menu rich-menu" aria-label={label}>
        {options.map((o) => (
          <SelectItem key={o.label} value={o.value} className="menu-item">
            <span className="menu-label" style={o.style}>
              {o.label}
            </span>
          </SelectItem>
        ))}
      </SelectPopover>
    </SelectProvider>
  );
}

/** Enter and Space on a toolbar button, as a click would be. Handled on the
 *  key itself rather than left to the click the browser synthesises: the
 *  command moves focus into the editor, and an Enter that arrived there
 *  afterwards would start a new paragraph. The buttons answered the mouse
 *  only, so the toolbar announced itself to the keyboard and then did
 *  nothing. */
function pressed(e: React.KeyboardEvent, run: () => void) {
  if (e.key !== 'Enter' && e.key !== ' ') return;
  e.preventDefault();
  run();
}

/** A character-level format: bold, italic and the rest. */
function Mark({
  editor, name, icon, label, keys, ...roving
}: {
  editor: Editor;
  name: string;
  icon: LucideIcon;
  label: string;
  keys?: string;
} & Roving) {
  const run = () => editor.chain().focus().toggleMark(name).run();
  return (
    <Tip label={label} keys={keys ? [keys] : undefined} placement="top">
      <button
        type="button"
        className={editor.isActive(name) ? 'rich-btn on' : 'rich-btn'}
        aria-label={label}
        aria-pressed={editor.isActive(name)}
        {...rove(roving)}
        // Mouse down, not click: a click would take focus out of the editor
        // first, collapsing the selection the button is meant to act on.
        onMouseDown={(e) => {
          e.preventDefault();
          run();
        }}
        onKeyDown={(e) => pressed(e, run)}
      >
        <Icon icon={icon} size={14} />
      </button>
    </Tip>
  );
}

/** A block-level format, or the link control. */
function Node({
  editor, name, icon, label, run, ...roving
}: {
  editor: Editor;
  name: string;
  icon: LucideIcon;
  label: string;
  run: () => void;
} & Roving) {
  return (
    <Tip label={label} placement="top">
      <button
        type="button"
        className={editor.isActive(name) ? 'rich-btn on' : 'rich-btn'}
        aria-label={label}
        aria-pressed={editor.isActive(name)}
        {...rove(roving)}
        onMouseDown={(e) => {
          e.preventDefault();
          run();
        }}
        onKeyDown={(e) => pressed(e, run)}
      >
        <Icon icon={icon} size={14} />
      </button>
    </Tip>
  );
}
