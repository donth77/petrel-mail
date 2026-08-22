import { useEffect, useMemo, useRef, useState } from 'react';
import { Check, Clock, FolderClosed, Plus, Tag as TagIcon, X } from 'lucide-react';
import {
  Combobox, ComboboxItem, ComboboxList, ComboboxProvider, Dialog, DialogDismiss,
} from '@ariakit/react';
import { fuzzyMatch, scoreMatch } from '../lib/commands';
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

/** Shows which characters earned the match, so the ordering never looks arbitrary. */
function Highlight({ text, hits }: { text: string; hits: number[] }) {
  if (hits.length === 0) return <>{text}</>;
  const set = new Set(hits);
  return (
    <>
      {[...text].map((ch, i) => (set.has(i) ? <span className="hit" key={i}>{ch}</span> : ch))}
    </>
  );
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

  const matches = useMemo(() => {
    const q = query.trim();
    if (!q) return options.map((o) => ({ o, hits: [] as number[] }));
    return options
      .map((o) => ({ o, hits: fuzzyMatch(q, o.label) }))
      .filter((m): m is { o: PickerOption; hits: number[] } => m.hits !== null)
      .sort((a, b) => scoreMatch(b.hits, b.o.label) - scoreMatch(a.hits, a.o.label));
  }, [options, query]);

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
      <ComboboxProvider setValue={setQuery} resetValueOnHide>
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
          {matches.map(({ o, hits }) => (
            <ComboboxItem
              key={o.id}
              className="picker-opt"
              focusOnHover
              // Tag mode stays open: applying two tags should not cost two
              // trips through the picker.
              hideOnClick={mode === 'folder'}
              onClick={() => onChoose(o.id, !o.on)}
            >
              {mode === 'snooze' ? (
                <Icon icon={Clock} size={13} />
              ) : mode === 'tag' ? (
                <span className={`picker-check${o.on ? ' on' : ''}`} aria-hidden="true">
                  {o.on && <Icon icon={Check} size={10} />}
                </span>
              ) : (
                <Icon icon={FolderClosed} size={13} />
              )}
              {o.colour && (
                <span className="picker-dot" aria-hidden="true" style={{ background: o.colour }} />
              )}
              <span className="clip">
                <Highlight text={o.label} hits={hits} />
              </span>
              {o.detail && <span className="picker-when mono">{o.detail}</span>}
            </ComboboxItem>
          ))}

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
