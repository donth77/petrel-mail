import { useMemo, useState } from 'react';
import {
  Archive, ChevronDown, ChevronRight, FolderClosed, Plus, Tag as TagIcon, Trash2,
} from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import {
  Combobox,
  ComboboxItem,
  ComboboxList,
  ComboboxProvider,
  Popover,
  PopoverDisclosure,
  PopoverProvider,
} from '@ariakit/react';
import { fuzzyMatch, scoreMatch } from '../lib/commands';
import { splitPath } from '../lib/folders';
import { useFolderFold } from '../lib/useFolderFold';
import { Highlight } from './Highlight';
import { PathLabel } from './PathLabel';
import { Icon } from './Icon';
import { t } from '../lib/strings';

export type FieldOption = {
  id: number;
  /** What a search matches against. For a nested folder this is the full path. */
  label: string;
  /** Tag colour, when there is one. */
  colour?: string;
  /** How deep in the folder tree, for the unfiltered list. */
  depth?: number;
  /** A rung of a path that is not itself a destination — drawn so a child is
   *  not indented under nothing, never offered as a choice. */
  container?: boolean;
  /** Whether anything hangs under it — which is whether it gets a chevron. */
  hasChildren?: boolean;
  /** Archive and Trash, which wear their mailbox's glyph and start folded. */
  anchor?: 'archive' | 'trash';
  /** A glyph of the row's own, for a list that is not only folders — the same
   *  escape hatch the dialog picker's options have. */
  icon?: LucideIcon;
};

/** Archive and Trash are mailboxes wearing a rung's clothes; the rest are folders. */
function glyphOf(o: FieldOption) {
  if (o.icon) return o.icon;
  if (o.anchor === 'archive') return Archive;
  if (o.anchor === 'trash') return Trash2;
  return FolderClosed;
}

/** Indentation for a tree row. Nothing at all for a list that has no depth. */
function indentOf(o: FieldOption) {
  return o.depth ? ({ paddingInlineStart: 14 + o.depth * 14 } as const) : undefined;
}

/**
 * One folder or one tag, chosen from a searchable dropdown.
 *
 * The same idea as the move and tag pickers — type to filter, the matched
 * characters shown, create the name you typed from the last row — anchored to
 * a field rather than opened as a dialog over the whole window. A rule is a
 * form with several decisions in it, and a modal that covers the rule while
 * you make one of them takes away what you are deciding against.
 *
 * It replaces a native `<select>`, which looked adequate and was not. A native
 * popup's type-ahead matches from the *start* of the option text, so with the
 * full paths these lists carry, `Archive/Yearly/2023` could only be reached by
 * typing `Archive/Yea…` — you could not find a folder by the part of its name
 * you actually remember. Fuzzy matching over the whole path fixes exactly that.
 */
export function PickerField({
  mode,
  label,
  value,
  options,
  noneLabel,
  onChange,
  onCreate,
}: {
  mode: 'folder' | 'tag';
  /** Names the control. It cannot come from a wrapping <label>: see the note
   *  on the disclosure below. */
  label: string;
  /** The chosen row, or null for "do not do this at all". */
  value: number | null;
  options: FieldOption[];
  /** What the empty choice reads as — "Nowhere", "No tag". */
  noneLabel: string;
  onChange: (id: number | null) => void;
  /** Makes the name that was typed and chooses it. Absent where creating from
   *  here would not make sense. */
  onCreate?: (name: string) => void;
}) {
  const [query, setQuery] = useState('');
  const [open, setOpen] = useState(false);

  // Idle it is a tree to browse — the engine's order, its depth, and each row
  // saying only its own name. Typed it is a ranked search, flat, every row
  // spelling its whole path so a deep match can say where it is. A filtered
  // tree can be neither: it either pads itself with ancestors that did not
  // match, or keeps an indentation that describes nothing. Containers belong
  // to the tree half for the same reason.
  const matches = useMemo(() => {
    const q = query.trim();
    if (!q) return options.map((o) => ({ o, hits: [] as number[] }));
    return options
      .filter((o) => !o.container)
      .map((o) => ({ o, hits: fuzzyMatch(q, o.label) }))
      .filter((m): m is { o: FieldOption; hits: number[] } => m.hits !== null)
      .sort((a, b) => scoreMatch(b.hits, b.o.label) - scoreMatch(a.hits, a.o.label));
  }, [options, query]);
  const browsing = query.trim().length === 0;
  const fold = useFolderFold(options);
  // Folding only means anything while the tree is on screen; a search is
  // already a filter, and hiding a match inside a folded parent would drop a
  // row for a reason no longer visible.
  const listed = browsing ? matches.filter(({ o }) => !fold.hidden(o.label)) : matches;

  // Offer creation only when the text is not already a name in the list —
  // otherwise the last row invites you to make a duplicate of what you can see.
  const typed = query.trim();
  const canCreate =
    onCreate !== undefined &&
    typed.length > 0 &&
    !options.some((o) => o.label.toLowerCase() === typed.toLowerCase());

  const chosen = value === null ? null : (options.find((o) => o.id === value) ?? null);
  const glyph = mode === 'folder' ? FolderClosed : TagIcon;

  /** Take the choice and put the list away. Every row goes through here. */
  const choose = (take: () => void) => {
    take();
    setOpen(false);
  };

  return (
    <PopoverProvider open={open} setOpen={setOpen} placement="bottom-start">
      {/* Named here rather than by a wrapping <label>, and that is load-bearing.
          Inside a label, every click in the popover — the popover being a
          descendant of it — is forwarded by the browser to the label's control,
          which is this button. So choosing a row set the value and then toggled
          the list straight back open. It reproduced only under real input:
          label forwarding is user activation, so a synthetic click looked fine. */}
      <PopoverDisclosure className="picker-field" aria-label={label} aria-haspopup="listbox">
        {chosen?.colour && (
          <span className="picker-dot" aria-hidden="true" style={{ background: chosen.colour }} />
        )}
        <span className="clip">{chosen ? chosen.label : noneLabel}</span>
        <Icon icon={ChevronDown} size={13} />
      </PopoverDisclosure>
      {/* unmountOnHide, or every field leaves a second search box in the
          document — two comboboxes with the same placeholder, one of them
          unreachable, which is the sort of thing that only surfaces in an
          accessibility tree or a test selector. */}
      <Popover gutter={5} sameWidth unmountOnHide className="picker-pop">
        {/* The combobox owns filtering and the active row; the popover owns
            whether any of this is on screen. Keeping those in separate stores
            is deliberate. Composed as a Select *with* a combobox, the choice
            registered and the list stayed open over the form: between the two
            components neither one's hideOnClick fires, and an onClick on the
            row is lost in the prop merge. Here there is exactly one thing that
            can close the list, and it is ours.

            `open` is pinned, exactly as the dialog picker pins it and for the
            same reason: left to itself the combobox shows its list only once
            the input has focus, and in WebKit focus inside a just-mounted
            layer does not land the way it does in Chromium — so the list
            renders and is never drawn. The list is the whole point. */}
        <ComboboxProvider open includesBaseElement={false} setValue={setQuery} resetValueOnHide>
          <div className="picker-head">
            <Icon icon={glyph} size={14} />
            <Combobox
              autoSelect
              autoFocus
              className="picker-input"
              placeholder={t(mode === 'folder' ? 'picker-folder-hint' : 'picker-tag-hint')}
              onKeyDown={(e) => {
                // ↵ with nothing matching means "make it" — the create row is
                // the active item otherwise, so only the empty list is special.
                if (e.key === 'Enter' && matches.length === 0 && canCreate) {
                  e.preventDefault();
                  choose(() => onCreate?.(typed));
                }
              }}
            />
          </div>
          <ComboboxList className="picker-list">
            {/* Doing nothing is a choice the rule can make, so it is a row in
                the list rather than a separate way to clear the field. */}
            {typed.length === 0 && (
              <ComboboxItem
                className="picker-opt picker-none"
                focusOnHover
                onClick={() => choose(() => onChange(null))}
              >
                <span className="clip">{noneLabel}</span>
              </ComboboxItem>
            )}
            {listed.map(({ o, hits }) =>
              o.container ? (
                // A rung, not a destination. Not a ComboboxItem, so it cannot
                // be chosen and arrowing steps straight past it.
                <div key={o.id} className="picker-opt tree-container" style={indentOf(o)}>
                  {o.hasChildren && (
                    <button
                      type="button"
                      className="tree-toggle"
                      aria-label={t(fold.isOpen(o.label) ? 'folder-fold' : 'folder-unfold')}
                      aria-expanded={fold.isOpen(o.label)}
                      onClick={(e) => {
                        // The row is a choice; the chevron is not.
                        e.stopPropagation();
                        fold.toggle(o.label);
                      }}
                      onPointerDown={(e) => e.stopPropagation()}
                    >
                      <Icon icon={fold.isOpen(o.label) ? ChevronDown : ChevronRight} size={12} />
                    </button>
                  )}
                  <Icon icon={glyphOf(o)} size={13} />
                  <span className="clip">{splitPath(o.label).leaf}</span>
                </div>
              ) : (
                <ComboboxItem
                  key={o.id}
                  className="picker-opt"
                  // Only while browsing. Ranked and flat, an indent describes
              // nothing — a lone deep match would sit inset under a parent
              // that is no longer on screen.
              style={browsing ? indentOf(o) : undefined}
                  focusOnHover
                  onClick={() => choose(() => onChange(o.id))}
                >
                  {browsing && mode === 'folder' && (
                    <>
                  {o.hasChildren && (
                    <button
                      type="button"
                      className="tree-toggle"
                      aria-label={t(fold.isOpen(o.label) ? 'folder-fold' : 'folder-unfold')}
                      aria-expanded={fold.isOpen(o.label)}
                      onClick={(e) => {
                        // The row is a choice; the chevron is not.
                        e.stopPropagation();
                        fold.toggle(o.label);
                      }}
                      onPointerDown={(e) => e.stopPropagation()}
                    >
                      <Icon icon={fold.isOpen(o.label) ? ChevronDown : ChevronRight} size={12} />
                    </button>
                  )}
                    </>
                  )}
                  {mode === 'folder' && <Icon icon={glyphOf(o)} size={13} />}
                  {o.colour && (
                    <span
                      className="picker-dot"
                      aria-hidden="true"
                      style={{ background: o.colour }}
                    />
                  )}
                  {mode !== 'folder' ? (
                    <span className="clip">
                      <Highlight text={o.label} hits={hits} />
                    </span>
                  ) : browsing ? (
                    <span className="clip">{splitPath(o.label).leaf}</span>
                  ) : (
                    <PathLabel path={o.label} hits={hits} />
                  )}
                </ComboboxItem>
              ),
            )}
            {canCreate && (
              <ComboboxItem
                className="picker-opt picker-create"
                focusOnHover
                onClick={() => choose(() => onCreate?.(typed))}
              >
                <Icon icon={Plus} size={13} />
                <span className="clip">
                  {t(mode === 'folder' ? 'picker-new-folder' : 'picker-new-tag', { name: typed })}
                </span>
              </ComboboxItem>
            )}
            {matches.length === 0 && !canCreate && (
              <div className="picker-empty">{t('picker-none')}</div>
            )}
          </ComboboxList>
        </ComboboxProvider>
      </Popover>
    </PopoverProvider>
  );
}
