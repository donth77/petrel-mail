import { useEffect, useMemo, useState } from 'react';
import { Mail, X } from 'lucide-react';
import {
  Combobox, ComboboxItem, ComboboxList, ComboboxProvider, Dialog, DialogDismiss,
} from '@ariakit/react';
import {
  buildCommands, fuzzyMatch, labelOf, nameOf, scoreMatch, suffixOf,
  type Command, type CommandContext,
} from '../lib/commands';
import { api, type Thread } from '../lib/api';
import { count as fmtCount, listTime } from '../lib/format';
import { Icon } from './Icon';
import { clickAway } from '../lib/dialog';
import { t } from '../lib/strings';

const GROUP_LABEL = {
  conversation: 'palette-group-conversation',
  goto: 'palette-group-goto',
  app: 'palette-group-app',
} as const;

/** Shows which characters earned the match, so ranking never looks arbitrary. */
function Highlight({ text, hits }: { text: string; hits: number[] }) {
  if (hits.length === 0) return <>{text}</>;
  const set = new Set(hits);
  return (
    <>
      {[...text].map((ch, i) =>
        set.has(i) ? (
          <span className="hit" key={i}>
            {ch}
          </span>
        ) : (
          ch
        ),
      )}
    </>
  );
}

type Props = {
  open: boolean;
  onClose: () => void;
  subject: string | null;
  ctx: CommandContext;
  /** Opening a conversation the palette found. */
  onOpen: (threadId: number) => void;
};

const VISIBLE_LIMIT = 8;
/** Enough to recognise the one you meant, not so many that the commands vanish
 *  under a list of mail. Everything else is one Enter away in the full search. */
const MAIL_LIMIT = 5;

export function Palette({ open, onClose, subject, ctx, onOpen }: Props) {
  const [query, setQuery] = useState('');
  const commands = useMemo(() => buildCommands(ctx), [ctx]);

  // Groups keep a fixed order and rank only within themselves. Letting groups
  // reorder by best match makes the list jump around under the cursor as you
  // type, which is worse than a slightly less optimal ordering.
  const SCOPES: Command['scope'][] = ['conversation', 'goto', 'app'];
  const matched = useMemo(() => {
    const out: { cmd: Command; hits: number[]; score: number }[] = [];
    commands.forEach((cmd, i) => {
      const label = labelOf(cmd);
      const hits = fuzzyMatch(query, label);
      if (hits) out.push({ cmd, hits, score: scoreMatch(hits, label) - i * 0.001 });
    });
    if (!query) return out;
    return out.sort(
      (a, b) =>
        SCOPES.indexOf(a.cmd.scope) - SCOPES.indexOf(b.cmd.scope) || b.score - a.score,
    );
  }, [commands, query]);

  // Mail, alongside the commands. The palette is where people already go to
  // find things by typing, and making them close it and aim at a different
  // box to search their mail is a distinction the app cares about and they do
  // not.
  const [mail, setMail] = useState<Thread[]>([]);
  useEffect(() => {
    const needle = query.trim();
    if (!open || needle.length < 2) {
      setMail([]);
      return;
    }
    let live = true;
    // Short debounce: this runs against the local index, but a keystroke is
    // faster than any query and there is no point starting one per character.
    const h = setTimeout(() => {
      api
        .search(needle)
        .then((rows) => live && setMail(rows.slice(0, MAIL_LIMIT)))
        .catch(() => live && setMail([]));
    }, 90);
    return () => {
      live = false;
      clearTimeout(h);
    };
  }, [query, open]);

  const shown = matched.slice(0, VISIBLE_LIMIT);
  const overflow = matched.length - shown.length;

  // Groups keep their declared order; within a query, ranking has already been
  // applied, so a group appears where its best match falls.
  const groups: { scope: Command['scope']; items: typeof shown }[] = [];
  for (const m of shown) {
    const last = groups[groups.length - 1];
    if (last && last.scope === m.cmd.scope) last.items.push(m);
    else groups.push({ scope: m.cmd.scope, items: [m] });
  }

  const close = () => {
    setQuery('');
    onClose();
  };

  return (
    <Dialog
      open={open}
      onClose={close}
      className="palette-backdrop"
      {...clickAway(onClose)}
      backdrop={<div className="palette-scrim" onClick={onClose} />}
      aria-label={t('palette-title')}
    >
      <ComboboxProvider
        open
        setValue={setQuery}
        resetValueOnHide={false}
        includesBaseElement={false}
      >
        <div className="palette">
          <div className="palette-input">
            <Combobox
              autoSelect
              autoFocus
              placeholder={t('palette-placeholder')}
              className="palette-field"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              // The webview offers its own autofill menu over any field it
              // thinks is a form, which lands on top of the palette's own list
              // and is not ours to style, dismiss or navigate.
              autoComplete="off"
              autoCorrect="off"
              spellCheck={false}
            />
              <DialogDismiss className="close-btn palette-esc" aria-label={t('close')}>
              <Icon icon={X} size={15} />
            </DialogDismiss>
          </div>

          <ComboboxList className="palette-list">
            {groups.map((g) => (
              <div key={g.scope}>
                <div className="grouplabel">
                  {t(GROUP_LABEL[g.scope])}
                  {/* Naming the target is what makes "Snooze" unambiguous. */}
                  {g.scope === 'conversation' && subject && (
                    <span className="grouplabel-target clip">{subject}</span>
                  )}
                </div>
                {g.items.map(({ cmd, hits }) => (
                  <ComboboxItem
                    key={cmd.id}
                    className="cmd"
                    focusOnHover
                    setValueOnClick={false}
                    onClick={() => {
                      cmd.run();
                      close();
                    }}
                  >
                    <span className="ico">
                      <Icon icon={cmd.icon} size={16} />
                    </span>
                    <span className="name clip">
                      <Highlight text={nameOf(cmd)} hits={hits.filter((h) => h < nameOf(cmd).length)} />
                      {suffixOf(cmd) && (
                        <span className="alias">
                          {/* Hits past the name belong to the suffix, offset so
                              highlighting lands on the right characters. */}
                          <Highlight
                            text={suffixOf(cmd)}
                            hits={hits
                              .filter((h) => h >= nameOf(cmd).length)
                              .map((h) => h - nameOf(cmd).length)}
                          />
                        </span>
                      )}
                    </span>
                    {cmd.keys?.map((k) => (
                      <span className="kbd" key={k}>
                        {k}
                      </span>
                    ))}
                  </ComboboxItem>
                ))}
              </div>
            ))}

            {mail.length > 0 && (
              <div>
                <div className="grouplabel">{t('palette-group-mail')}</div>
                {mail.map((m) => (
                  <ComboboxItem
                    key={m.thread_id}
                    className="cmd"
                    focusOnHover
                    setValueOnClick={false}
                    onClick={() => {
                      onOpen(m.thread_id);
                      close();
                    }}
                  >
                    <span className="ico">
                      <Icon icon={Mail} size={16} />
                    </span>
                    <span className="name clip">
                      {m.subject || t('no-subject')}
                      <span className="alias">
                        {' · '}
                        {m.from_display || m.from_addr}
                      </span>
                    </span>
                    <span className="mono palette-when">{listTime(m.date_ms)}</span>
                  </ComboboxItem>
                ))}
              </div>
            )}

            {matched.length === 0 && mail.length === 0 && (
              <div className="palette-none">{t('palette-empty', { query })}</div>
            )}
          </ComboboxList>

          <div className="palette-foot">
            {overflow > 0 && (
              <span className="mono">{t('palette-more', { count: fmtCount(overflow) })}</span>
            )}
            <span className="palette-foot-spacer" />
            <span>
              <span className="kbd">↑</span> <span className="kbd">↓</span> {t('palette-navigate')}
            </span>
            <span>
              <span className="kbd">↵</span> {t('palette-run')}
            </span>
            <span>
              <span className="kbd">?</span> {t('palette-all-shortcuts')}
            </span>
          </div>
        </div>
      </ComboboxProvider>
    </Dialog>
  );
}
