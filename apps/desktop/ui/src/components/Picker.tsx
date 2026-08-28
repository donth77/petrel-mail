import { useEffect, useMemo, useRef, useState } from 'react';
import {
  Archive, Check, ChevronDown, ChevronRight, Clock, FolderClosed, Plus, Tag as TagIcon,
  Trash2, X,
} from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import {
  Combobox, ComboboxItem, ComboboxList, ComboboxProvider, Dialog, DialogDismiss,
} from '@ariakit/react';
import { fuzzyMatch, scoreMatch } from '../lib/commands';
import { splitPath } from '../lib/folders';
import { useFolderFold } from '../lib/useFolderFold';
import { Highlight } from './Highlight';
import { PathLabel } from './PathLabel';
import { Icon } from './Icon';
import { t } from '../lib/strings';

export type PickerOption = {
  /** For folders and tags this is a row id; for snooze it is the instant to
   *  come back at, which is the only thing that identifies the choice. */
  id: number;
  /** What the user reads. For a nested folder this is the full path. */
  label: string;
  /** Tag colour, when there is one. */
  colour?: string;
  /** Already applied — tag mode only, where the list is a set of checkboxes. */
  on?: boolean;
  /** The resolved time, shown beside a snooze preset: "Tomorrow" means nothing
   *  without "Thu 8:00 AM" next to it. */
  detail?: string;
  /** Folder mode only: a glyph that says what kind of place this is — the
   *  pinned Archive and Trash rows wear their own rather than a folder's. */
  icon?: LucideIcon;
  /** How deep in the folder tree, for the unfiltered list. */
  depth?: number;
  /** A rung of a path that is not itself a destination — drawn so a child is
   *  not indented under nothing, never offered as a choice. */
  container?: boolean;
  /** Whether anything hangs under it — which is whether it gets a chevron. */
  hasChildren?: boolean;
  /** Archive and Trash, which wear their mailbox's glyph and start folded. */
  anchor?: 'archive' | 'trash';
};

type Props = {
  open: boolean;
  /** True on Gmail, where what IMAP calls folders are labels. */
  labelsNotFolders?: boolean;
  /** Move is a single choice that closes; tag is a set you toggle. */
  mode: 'folder' | 'tag' | 'snooze';
  options: PickerOption[];
  subject: string | null;
  onClose: () => void;
  onChoose: (id: number, on: boolean) => void;
  onCreate: (name: string) => void;
};

/** Indentation for a tree row. Nothing at all for a list that has no depth. */
function indentOf(o: { depth?: number }) {
  return o.depth ? ({ paddingInlineStart: 14 + o.depth * 14 } as const) : undefined;
}

/** Archive and Trash are mailboxes wearing a rung's clothes; the rest are folders. */
function glyphOf(o: { anchor?: 'archive' | 'trash'; icon?: LucideIcon }) {
  if (o.icon) return o.icon;
  if (o.anchor === 'archive') return Archive;
  if (o.anchor === 'trash') return Trash2;
  return FolderClosed;
}

/**
 * The move and tag pickers, which are the same control with two selection
 * models.
 *
 * Typing filters and ↵ takes the top match, so the common case — a folder you
 * already know the name of — is one keystroke and a few letters rather than a
 * scroll hunt through a list that may hold hundreds. Creating is the last row of
 * the same list rather than a separate menu, because "file this under a name
 * that does not exist yet" is the same intent as "file this", and making it a
 * different gesture is what pushes people back to the mouse.
 */
export function Picker({ open, mode, options, subject, onClose, onChoose, onCreate , labelsNotFolders }: Props) {
  const [query, setQuery] = useState('');
  const inputRef = useRef<HTMLInputElement>(null);

  // A picker that remembers last time's filter is a picker that shows you the
  // wrong list the moment you open it again.
  useEffect(() => {
    if (open) setQuery('');
  }, [open]);

  // Two lists, and which one you get is decided by whether you have typed.
  //
  // Idle, this is a place to browse, so it keeps the tree's order and depth and
  // each row says only its own name — which is what indentation is for. The
  // moment there is a query it becomes a place to search: ranked by match,
  // every row spelling its whole path, because a match three levels down has
  // to say where it is. A filtered tree can do neither honestly — it either
  // drags in ancestors that did not match, padding the list its whole job is
  // to shorten, or keeps an indentation that no longer describes anything.
  //
  // Containers go with the tree. They exist to stop a child being indented
  // under nothing, and once the list is flat and ranked there is no nothing
  // for it to be indented under.
  const matches = useMemo(() => {
    const q = query.trim();
    if (!q) return options.map((o) => ({ o, hits: [] as number[] }));
    return options
      .filter((o) => !o.container)
      .map((o) => ({ o, hits: fuzzyMatch(q, o.label) }))
      .filter((m): m is { o: PickerOption; hits: number[] } => m.hits !== null)
      .sort((a, b) => scoreMatch(b.hits, b.o.label) - scoreMatch(a.hits, a.o.label));
  }, [options, query]);
  const browsing = query.trim().length === 0;
  const fold = useFolderFold(options);
  // Folding only means anything while the tree is on screen. A search is
  // already a filter, and hiding matches inside a folded parent would drop
  // rows that matched for a reason that is no longer visible.
  const listed = browsing ? matches.filter(({ o }) => !fold.hidden(o.label)) : matches;

  // Offer creation only when the text is not already a name in the list —
  // otherwise the last row invites you to make a duplicate of what you can see.
  const typed = query.trim();
  // Snooze offers fixed times; there is nothing to create.
  const canCreate =
    mode !== 'snooze' &&
    typed.length > 0 &&
    !options.some((o) => o.label.toLowerCase() === typed.toLowerCase());

  return (
    <Dialog
      open={open}
      onClose={onClose}
      backdrop={<div className="palette-backdrop" />}
      className="picker"
      aria-label={t(
        mode === 'folder' ? 'picker-folder-title' : mode === 'tag' ? 'picker-tag-title' : 'picker-snooze-title',
      )}
    >
      {/* `open` is pinned, exactly as the palette pins it. Left to itself the
          combobox shows its list only once the input has focus, and in WebKit
          focus inside a dialog that has just mounted does not land the way it
          does in Chromium — so the list stayed `display: none`: four tag rows
          in the DOM, none of them drawn, a dialog with a search box and nothing
          to search. The list is the whole point of this dialog; it is never
          closed while the dialog is. */}
      <ComboboxProvider open setValue={setQuery} resetValueOnHide>
        <div className="picker-head">
          <Icon icon={mode === 'folder' ? FolderClosed : mode === 'tag' ? TagIcon : Clock} size={14} />
          <Combobox
            ref={inputRef}
            autoSelect
            autoFocus
            className="picker-input"
            placeholder={t(
              mode === 'folder' ? 'picker-folder-hint' : mode === 'tag' ? 'picker-tag-hint' : 'picker-snooze-hint',
            )}
            onKeyDown={(e) => {
              // ↵ with nothing matching means "make it" — the create row is the
              // active item in that case, so only the empty-list case is special.
              if (e.key === 'Enter' && matches.length === 0 && canCreate) {
                e.preventDefault();
                onCreate(typed);
              }
            }}
          />
            <DialogDismiss className="close-btn" aria-label={t('close')}>
              <Icon icon={X} size={15} />
            </DialogDismiss>
        </div>

        {subject && <div className="picker-subject clip">{subject}</div>}

        <ComboboxList className="picker-list">
          {listed.map(({ o, hits }) =>
            o.container ? (
              // A rung, not a destination: drawn at its depth so what hangs
              // under it reads as hanging under it, and deliberately not a
              // ComboboxItem, so it cannot be chosen and arrowing steps past.
              <div key={o.id} className="picker-opt tree-container" style={indentOf(o)}>
                {o.hasChildren && (
                  // The chevron hangs in the row's own indent, so an icon
                  // holds the same column whether or not its row folds.
                  <button
                    type="button"
                    className="tree-toggle"
                    aria-label={t(fold.isOpen(o.label) ? 'folder-fold' : 'folder-unfold')}
                    aria-expanded={fold.isOpen(o.label)}
                    onClick={(e) => {
                      // The row is a choice; the chevron is not. Without this,
                      // opening Archive files the mail into it.
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
              // Tag mode stays open: applying two tags should not cost two
              // trips through the picker.
              hideOnClick={mode === 'folder'}
              onClick={() => onChoose(o.id, !o.on)}
            >
              {browsing && mode === 'folder' && (
                <>
                {o.hasChildren && (
                  // The chevron hangs in the row's own indent, so an icon
                  // holds the same column whether or not its row folds.
                  <button
                    type="button"
                    className="tree-toggle"
                    aria-label={t(fold.isOpen(o.label) ? 'folder-fold' : 'folder-unfold')}
                    aria-expanded={fold.isOpen(o.label)}
                    onClick={(e) => {
                      // The row is a choice; the chevron is not. Without this,
                      // opening Archive files the mail into it.
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
              {mode === 'snooze' ? (
                <Icon icon={Clock} size={13} />
              ) : mode === 'tag' ? (
                <span className={`picker-check${o.on ? ' on' : ''}`} aria-hidden="true">
                  {o.on && <Icon icon={Check} size={10} />}
                </span>
              ) : (
                <Icon icon={glyphOf(o)} size={13} />
              )}
              {o.colour && (
                <span className="picker-dot" aria-hidden="true" style={{ background: o.colour }} />
              )}
              {/* A folder is a path, and a path has a part worth keeping when
                  the row runs out of room. A tag or a snooze time is one word
                  and reads as itself. */}
              {mode !== 'folder' ? (
                <span className="clip">
                  <Highlight text={o.label} hits={hits} />
                </span>
              ) : browsing ? (
                // Its own name only. The indentation is already saying where
                // it sits, and repeating the parents beside it is the noise
                // the indentation was drawn to remove.
                <span className="clip">{splitPath(o.label).leaf}</span>
              ) : (
                <PathLabel path={o.label} hits={hits} />
              )}
              {o.detail && <span className="picker-when mono">{o.detail}</span>}
            </ComboboxItem>
            ),
          )}

          {canCreate && (
            <ComboboxItem
              className="picker-opt picker-create"
              focusOnHover
              hideOnClick
              onClick={() => onCreate(typed)}
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

        <div className="picker-foot">
          {t(
            mode === 'folder'
              ? labelsNotFolders
                ? 'picker-folder-foot-labels'
                : 'picker-folder-foot'
              : mode === 'tag'
                ? 'picker-tag-foot'
                : 'picker-snooze-foot',
          )}
        </div>
      </ComboboxProvider>
    </Dialog>
  );
}
