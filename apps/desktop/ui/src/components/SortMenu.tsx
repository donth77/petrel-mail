import { Menu, MenuButton, MenuItem, MenuProvider, MenuSeparator } from '@ariakit/react';
import { ArrowDown, ArrowUp, Check } from 'lucide-react';
import { Icon } from './Icon';
import { t } from '../lib/strings';
import { KEY_LABEL, directionLabels, sortKeys, type Sort, type SortKey } from '../lib/sort';

/**
 * How the list is ordered, in the header row beside the mailbox's name.
 *
 * One control for the mailbox and for search results. They had different
 * answers before — a search grew two buttons of its own and a mailbox had no
 * control at all — so the same question was asked two ways depending on
 * whether the box above happened to have anything in it.
 *
 * A menu rather than a row of buttons because there are now four keys and two
 * directions: eight buttons is a toolbar, and this is a preference somebody
 * sets and forgets.
 */
export function SortMenu({
  sort,
  onChange,
  searching,
}: {
  sort: Sort;
  onChange: (sort: Sort) => void;
  /** Whether a query is running, which is the only time relevance exists. */
  searching: boolean;
}) {
  const dir = directionLabels(sort.key);
  return (
    <MenuProvider placement="bottom-end">
      <MenuButton
        className="sort-btn"
        aria-label={t('sort-by')}
      >
        <span className="sort-btn-label">{t(KEY_LABEL[sort.key])}</span>
        {/* Relevance has no direction: it is the order the ranking produced,
            and reversing it would put the worst matches first. */}
        {sort.key !== 'relevance' && (
          <Icon icon={sort.ascending ? ArrowUp : ArrowDown} size={12} />
        )}
      </MenuButton>
      <Menu portal gutter={6} className="menu" aria-label={t('sort-by')}>
        {sortKeys(searching).map((key: SortKey) => (
          <MenuItem
            key={key}
            className="menu-item"
            aria-checked={key === sort.key}
            onClick={() => onChange({ ...sort, key })}
          >
            {/* The tick holds a column whether or not it is drawn, so the
                labels line up rather than shifting by 14px as the choice
                moves down the list. */}
            <span className="menu-tick">
              {key === sort.key && <Icon icon={Check} size={13} />}
            </span>
            <span className="menu-label">{t(KEY_LABEL[key])}</span>
          </MenuItem>
        ))}
        {sort.key !== 'relevance' && (
          <>
            <MenuSeparator className="menu-sep" />
            <MenuItem
              className="menu-item"
              aria-checked={!sort.ascending}
              onClick={() => onChange({ ...sort, ascending: false })}
            >
              <span className="menu-tick">
                {!sort.ascending && <Icon icon={Check} size={13} />}
              </span>
              <span className="menu-label">{t(dir.descending)}</span>
            </MenuItem>
            <MenuItem
              className="menu-item"
              aria-checked={sort.ascending}
              onClick={() => onChange({ ...sort, ascending: true })}
            >
              <span className="menu-tick">
                {sort.ascending && <Icon icon={Check} size={13} />}
              </span>
              <span className="menu-label">{t(dir.ascending)}</span>
            </MenuItem>
          </>
        )}
      </Menu>
    </MenuProvider>
  );
}
