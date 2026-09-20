import { useMemo, useState } from 'react';
import { Dialog, DialogDismiss, Tab, TabList, TabPanel, useTabStore } from '@ariakit/react';
import { Search, X } from 'lucide-react';
import { operatorGroups, shortcutGroups } from '../lib/help';
import { Icon } from './Icon';
import { GithubMark } from './GithubMark';
import { api } from '../lib/api';
import { clickAway } from '../lib/dialog';
import { t } from '../lib/strings';
import { useSettings } from '../lib/settings';
import { ISSUES_URL, SOURCE_URL } from '../lib/project';

export function Help({ open, onClose }: { open: boolean; onClose: () => void }) {
  const tabs = useTabStore({ defaultSelectedId: 'shortcuts' });
  const [filter, setFilter] = useState('');
  const selected = tabs.useState('selectedId');
  // The shortcut table is built once per language, not once per mount:
  // shortcutGroups() resolves every label through t().
  const { locale } = useSettings();
  const groups = useMemo(() => shortcutGroups(), [locale]);

  const q = filter.trim().toLowerCase();
  const filteredShortcuts = useMemo(
    () =>
      groups
        .map((g) => ({
          ...g,
          rows: q ? g.rows.filter((r) => r.label.toLowerCase().includes(q)) : g.rows,
        }))
        .filter((g) => g.rows.length > 0),
    [groups, q],
  );
  const filteredOps = useMemo(
    () =>
      operatorGroups().map((g) => ({
        ...g,
        ops: q
          ? g.ops.filter(
              (o) =>
                o.op.toLowerCase().includes(q) || o.means.toLowerCase().includes(q),
            )
          : g.ops,
      })).filter((g) => g.ops.length > 0),
    [q, locale],
  );
  const opColumns = useMemo(
    () =>
      [
        filteredOps.filter((g) => g.side === 'left'),
        filteredOps.filter((g) => g.side === 'right'),
      ].filter((column) => column.length > 0),
    [filteredOps],
  );

  const close = () => {
    setFilter('');
    onClose();
  };

  return (
    <Dialog
      open={open}
      onClose={close}
      className="help-backdrop"
      {...clickAway(onClose)}
      backdrop={<div className="palette-scrim" onClick={onClose} />}
      aria-label={t('rail-help')}
    >
      <div className="help">
        <div className="help-head">
          <span className="help-title">{t('help-title')}</span>
          <TabList store={tabs} className="help-tabs">
            <Tab id="shortcuts" store={tabs} className="tab">
              {t('help-tab-shortcuts')}
            </Tab>
            <Tab id="search" store={tabs} className="tab">
              {t('help-tab-search')}
            </Tab>
          </TabList>
          <span className="help-spacer" />
          <label className="help-filter">
            <Search size={13} strokeWidth={1.8} aria-hidden="true" />
            <input
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
              placeholder={
                selected === 'search' ? t('help-filter-search') : t('help-filter-shortcuts')
              }
              aria-label={t('help-filter-shortcuts')}
            />
          </label>
            <DialogDismiss className="close-btn" aria-label={t('close')}>
              <Icon icon={X} size={15} />
            </DialogDismiss>
        </div>

        <TabPanel store={tabs} tabId="shortcuts" className="help-panel keys">
          {filteredShortcuts.map((g) => (
            <div key={g.title}>
              <div className="grp">{g.title}</div>
              {g.rows.map((r) => (
                <div className="line" key={r.label}>
                  <span className="lbl">{r.label}</span>
                  <span className="keys">
                    {r.keys.map((k) => (
                      <span className="kbd" key={k}>
                        {k}
                      </span>
                    ))}
                  </span>
                </div>
              ))}
            </div>
          ))}
          {filteredShortcuts.length === 0 && (
            <div className="palette-none">{t('palette-empty', { query: filter })}</div>
          )}
        </TabPanel>

        {/* Two columns that each pack their own groups. As one grid the rows
            were as tall as their taller cell, so the short group beside
            "Combining terms" left a hole under itself and the panel scrolled
            for no reason. When a filter leaves one side empty, what is left
            takes the first column rather than sitting beside a blank one. */}
        <TabPanel store={tabs} tabId="search" className="help-panel ops">
          {opColumns.map((column, i) => (
            <div className="ops-col" key={i}>
              {column.map((g) => (
                <div key={g.title}>
                  <div className="grp">{g.title}</div>
                  {g.ops.map((o) => (
                    <div className="op" key={o.op}>
                      <code>{o.op}</code>
                      <span>
                        {o.means}
                        {o.means && o.example ? ': ' : ''}
                        {o.example && <code className="bare">{o.example}</code>}
                      </span>
                    </div>
                  ))}
                </div>
              ))}
              {/* Two examples, because the second half of the grammar cannot
                  be shown by the first: one query that narrows by piling
                  conditions up, and one that asks a question with a shape —
                  either sender, this word, not that one. */}
              {i === opColumns.length - 1 && !q && (
                <div className="op-example">
                  <div className="mono op-example-query">
                    from:sam has:attachment after:2026-06-01 annex
                  </div>
                  <div className="op-example-note">{t('help-together-note')}</div>
                  <div className="mono op-example-query op-example-next">
                    (from:sam OR from:dana) invoice NOT draft
                  </div>
                  <div className="op-example-note">{t('help-boolean-note')}</div>
                </div>
              )}
            </div>
          ))}
          {filteredOps.length === 0 && (
            <div className="palette-none">{t('palette-empty', { query: filter })}</div>
          )}
        </TabPanel>

        {/* Outside the tabs on purpose: reporting a problem is not a thing you
            look up under Shortcuts or Search, and someone who wants it wants it
            from wherever they happen to be. */}
        <div className="help-foot">
          <button
            type="button"
            className="fbtn"
            onClick={() => void api.openExternal(ISSUES_URL)}
          >
            <GithubMark size={13} />
            {t('help-report')}
          </button>
          <button
            type="button"
            className="linkish help-source"
            onClick={() => void api.openExternal(SOURCE_URL)}
          >
            {t('help-source')}
          </button>
        </div>
      </div>
    </Dialog>
  );
}
