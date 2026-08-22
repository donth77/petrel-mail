import { useMemo, useState } from 'react';
import { Dialog, DialogDismiss, Tab, TabList, TabPanel, useTabStore } from '@ariakit/react';
import { Search, X } from 'lucide-react';
import { OPERATOR_GROUPS, shortcutGroups } from '../lib/help';
import { Icon } from './Icon';
import { Tip } from './Tip';
import { clickAway } from '../lib/dialog';
import { t } from '../lib/strings';

export function Help({ open, onClose }: { open: boolean; onClose: () => void }) {
  const tabs = useTabStore({ defaultSelectedId: 'shortcuts' });
  const [filter, setFilter] = useState('');
  const selected = tabs.useState('selectedId');
  const groups = useMemo(() => shortcutGroups(), []);

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
      OPERATOR_GROUPS.map((g) => ({
        ...g,
        ops: q
          ? g.ops.filter(
              (o) =>
                o.op.toLowerCase().includes(q) || o.means.toLowerCase().includes(q),
            )
          : g.ops,
      })).filter((g) => g.ops.length > 0),
    [q],
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
          <Tip label={t('close-title')} placement="bottom">
            <DialogDismiss className="close-btn" aria-label={t('close')}>
              <Icon icon={X} size={15} />
            </DialogDismiss>
          </Tip>
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

        <TabPanel store={tabs} tabId="search" className="help-panel ops">
          {filteredOps.map((g) => (
            <div key={g.title}>
              <div className="grp">{g.title}</div>
              {g.ops.map((o) => (
                <div className="op" key={o.op}>
                  <code>{o.op}</code>
                  <span>
                    {o.means}
                    {o.means && o.example ? ' — ' : ''}
                    {o.example && <code className="bare">{o.example}</code>}
                  </span>
                </div>
              ))}
            </div>
          ))}
          {filteredOps.length > 0 && !q && (
            <div className="op-example">
              <div className="op-example-label">{t('help-together')}</div>
              <div className="mono op-example-query">
                from:sam has:attachment after:2026-06-01 annex
              </div>
              <div className="op-example-note">{t('help-together-note')}</div>
            </div>
          )}
          {filteredOps.length === 0 && (
            <div className="palette-none">{t('palette-empty', { query: filter })}</div>
          )}
        </TabPanel>

      </div>
    </Dialog>
  );
}
