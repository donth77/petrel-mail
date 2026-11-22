import { useEffect, useImperativeHandle, useRef, useState } from 'react';
import { X } from 'lucide-react';
import { api, type Correspondent } from '../lib/api';
import { Icon } from './Icon';
import { t } from '../lib/strings';
import { looksLikeAddress, splitRecipients } from '../lib/recipients';

type Props = {
  label: string;
  value: string;
  onChange: (value: string) => void;
  inputRef?: React.Ref<HTMLInputElement>;
  /** The composer's way of asking for what is still being typed. */
  handle?: React.Ref<RecipientsHandle>;
  /** Begins a drag of one confirmed chip. The composer knows which field
   *  this is. A press on the × is still a click: the drag refuses a button. */
  onChipPointerDown?: (event: React.PointerEvent, index: number, addr: string) => void;
};

/** What the composer can ask of a field it does not own the state of. */
export type RecipientsHandle = {
  /** Commits whatever is still in the input and returns the field's new
   *  value, or null when nothing was pending. The composer reads the value
   *  back rather than waiting for onChange: a send in the same keystroke
   *  would otherwise go out with the address still in the input. */
  flush: () => string | null;
};

/**
 * A recipient field: committed addresses as chips, one live input at the end.
 *
 * The plain text field it replaces was not wrong so much as unhelpful. Six
 * recipients in one comma-separated line cannot be scanned, removing the third
 * one means text surgery, and a typo looks exactly like a good address until
 * the send fails. A chip is a thing you can see, count, and delete.
 *
 * The field stays a string on the outside. Drafts, reply prefill and the send
 * path all speak comma-separated recipients, and giving this component its own
 * array type would mean converting at every boundary and getting it wrong at
 * one of them.
 */
export function Recipients({ label, value, onChange, inputRef, handle, onChipPointerDown }: Props) {
  const [typed, setTyped] = useState('');
  const [options, setOptions] = useState<Correspondent[]>([]);
  const [highlight, setHighlight] = useState(0);
  const box = useRef<HTMLDivElement>(null);

  const chips = splitRecipients(value);

  // Read by flush, which runs from the composer's key handler in the same
  // event as the keystroke that filled them — before this render's closures
  // have caught up.
  const typedRef = useRef(typed);
  typedRef.current = typed;
  const valueRef = useRef(value);
  valueRef.current = value;
  useImperativeHandle(
    handle,
    () => ({
      flush: () => {
        const pending = typedRef.current.trim();
        if (!pending) return null;
        setTyped('');
        setOptions([]);
        return [...splitRecipients(valueRef.current), pending].join(', ');
      },
    }),
    [],
  );

  // Split on the way in, so a pasted list joins the field as one recipient per
  // entry and the value never carries the line breaks it arrived with.
  const commit = (addr: string): string => {
    const next = [...chips, ...splitRecipients(addr)].join(', ');
    onChange(next);
    setTyped('');
    setOptions([]);
    return next;
  };

  // The field as it stood before a pasted column became chips, and what the
  // paste made of it. The paste never reaches the input, so the input's own
  // undo knows nothing of it, and taking back a column of thirty took thirty
  // Backspaces. ⌘Z takes it back while the field still holds what it made.
  const beforePaste = useRef<{ value: string; typed: string; made: string } | null>(null);

  // The chip's button goes with the chip, and focus with it, to the body,
  // where the next keystroke is a shortcut. Back into the field, where the
  // person still is.
  const entry = useRef<HTMLInputElement | null>(null);
  const removeAt = (i: number) => {
    onChange(chips.filter((_, n) => n !== i).join(', '));
    entry.current?.focus();
  };

  // Suggestions come from mail already synced, so this is a local query and can
  // run on every keystroke without a debounce being a kindness to anyone's
  // network. It is still guarded: an empty prefix would ask for everybody.
  useEffect(() => {
    let live = true;
    const needle = typed.trim();
    if (!needle) {
      setOptions([]);
      return;
    }
    api
      .completeAddresses(needle)
      .then((rows) => {
        if (!live) return;
        // Never offer someone already in the field. A suggestion you cannot
        // usefully accept is noise in a list of eight.
        setOptions(rows.filter((r) => !chips.includes(r.addr)));
        setHighlight(0);
      })
      .catch(() => {});
    return () => {
      live = false;
    };
    // chips is derived from value; depending on the string avoids a new array
    // identity re-running this on every render.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [typed, value]);

  return (
    <div className="recipients" ref={box}>
      {/* `rcpt-chip`, not `chip`: the list header already owns that class, and
          reusing it here silently restyled the account pill above the list. */}
      {chips.map((addr, i) => (
        <span
          className={looksLikeAddress(addr) ? 'rcpt-chip' : 'rcpt-chip suspect'}
          key={`${addr}-${i}`}
          // The × is a button inside this span, and the drag refuses a press
          // that began there, so every press on the chip can be offered.
          onPointerDown={(event) => onChipPointerDown?.(event, i, addr)}
        >
          <span className="clip">{addr}</span>
          <button
            type="button"
            className="rcpt-x"
            aria-label={t('recipient-remove', { addr })}
            onClick={() => removeAt(i)}
          >
            <Icon icon={X} size={11} />
          </button>
        </span>
      ))}

      <div className="recipient-entry">
        <input
          ref={(el) => {
            entry.current = el;
            if (typeof inputRef === 'function') inputRef(el);
            else if (inputRef) (inputRef as { current: HTMLInputElement | null }).current = el;
          }}
          className="compose-input"
          value={typed}
          aria-label={label}
          autoComplete="off"
          spellCheck={false}
          onChange={(e) => setTyped(e.target.value)}
          onPaste={(e) => {
            // A column of addresses arrives one per line, and a one-line field
            // turns the breaks into spaces before anything here sees them: the
            // whole column became one chip. So a paste with a line break in it
            // is read while the breaks are still there, and committed as the
            // recipients it holds, together with whatever was already typed
            // around the caret.
            const text = e.clipboardData.getData('text/plain');
            if (!/[\r\n]/.test(text)) return;
            e.preventDefault();
            const el = e.currentTarget;
            const from = el.selectionStart ?? typed.length;
            const to = el.selectionEnd ?? from;
            const made = commit(typed.slice(0, from) + text + typed.slice(to));
            beforePaste.current = { value, typed, made };
          }}
          onBlur={() => {
            // Leaving the field commits what was typed. Losing a half-typed
            // address because you clicked the body is the kind of small theft
            // that makes people distrust the whole form.
            if (typed.trim()) commit(typed);
          }}
          onKeyDown={(e) => {
            const undo = beforePaste.current;
            if (
              undo &&
              e.key.toLowerCase() === 'z' &&
              (e.metaKey || e.ctrlKey) &&
              !e.shiftKey &&
              !e.altKey &&
              !typed &&
              valueRef.current === undo.made
            ) {
              e.preventDefault();
              beforePaste.current = null;
              onChange(undo.value);
              setTyped(undo.typed);
              return;
            }
            if (options.length > 0 && (e.key === 'ArrowDown' || e.key === 'ArrowUp')) {
              e.preventDefault();
              setHighlight((h) =>
                e.key === 'ArrowDown'
                  ? (h + 1) % options.length
                  : (h - 1 + options.length) % options.length,
              );
              return;
            }
            // Enter with a modifier is the composer's — send, or send later.
            // It flushes this field itself, so committing here as well would
            // do the same work twice in one keystroke.
            if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) return;
            // Enter takes the highlighted suggestion when the list is open, and
            // otherwise commits exactly what was typed. Tab and comma always
            // commit what was typed — a completion list must never put an
            // address you did not choose into a message you are about to send.
            if (e.key === 'Enter' && options.length > 0) {
              e.preventDefault();
              commit(options[highlight].addr);
              return;
            }
            if (e.key === 'Enter' || e.key === ',' || e.key === ';' || e.key === 'Tab') {
              if (!typed.trim()) return;
              // Tab still moves on afterwards; the others have nowhere to go.
              if (e.key !== 'Tab') e.preventDefault();
              commit(typed);
              return;
            }
            if (e.key === 'Escape' && options.length > 0) {
              e.preventDefault();
              setOptions([]);
              return;
            }
            // Backspace at the start of an empty input removes the last chip,
            // which is what every other field of this shape does.
            if (e.key === 'Backspace' && !typed && chips.length > 0) {
              e.preventDefault();
              removeAt(chips.length - 1);
            }
          }}
        />

        {options.length > 0 && (
          <ul className="completions" role="listbox" aria-label={t('recipient-suggestions')}>
            {options.map((o, i) => (
              <li key={o.addr}>
                <button
                  type="button"
                  role="option"
                  aria-selected={i === highlight}
                  className={i === highlight ? 'on' : undefined}
                  // Mouse down rather than click: the input's blur would
                  // otherwise commit the typed text and close the list before
                  // the click ever landed.
                  onMouseDown={(e) => {
                    e.preventDefault();
                    commit(o.addr);
                  }}
                  onMouseEnter={() => setHighlight(i)}
                >
                  <span className="completion-name clip">{o.display || o.addr}</span>
                  {o.display && <span className="completion-addr mono clip">{o.addr}</span>}
                  {o.written_to && (
                    <span className="completion-note">{t('recipient-written-to')}</span>
                  )}
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
