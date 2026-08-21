import { useMemo, useState } from 'react';
import {
  Combobox, ComboboxItem, ComboboxList, ComboboxProvider, Dialog, DialogDismiss,
} from '@ariakit/react';
import {
  buildCommands, fuzzyMatch, labelOf, nameOf, scoreMatch, suffixOf,
  type Command, type CommandContext,
} from '../lib/commands';
import { count as fmtCount } from '../lib/format';
import { Icon } from './Icon';
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
};

const VISIBLE_LIMIT = 8;

export function Palette({ open, onClose, subject, ctx }: Props) {
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
      backdrop={<div className="palette-scrim" />}
      aria-label={t('palette-placeholder')}
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
            />
            <DialogDismiss className="kbd palette-esc">esc</DialogDismiss>
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

            {matched.length === 0 && (
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
