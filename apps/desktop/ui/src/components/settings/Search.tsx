import { useSettings } from '../../lib/settings';
import { Pill } from './Pill';
import { t } from '../../lib/strings';

/**
 * How the search field behaves: what it writes for you, what it shows you,
 * and what it marks.
 *
 * Its own pane rather than three switches under Appearance, because only one
 * of the three is about how anything looks. Searching is a thing this app
 * does, with a grammar of its own and a Help tab of its own, and the window
 * makes decisions on your behalf while you type — scoping a search to the
 * mailbox you are in, offering the filter buttons, marking what it found.
 * Each of those is worth being able to turn off, and they belong together.
 */
export function Search() {
  const { settings, set } = useSettings();

  return (
    <div className="pane-body">
      <h1 className="pane-title">{t('settings-search')}</h1>

      {/* On first in each pill: these are all on unless somebody turns them
          off, and a pill reads left to right as "the usual, then the other". */}
      <section className="field">
        <div className="flabel">{t('search-in-mailbox')}</div>
        <p className="fhelp">{t('search-in-mailbox-help')}</p>
        <Pill
          label={t('search-in-mailbox')}
          value={settings.searchInMailbox}
          onChange={(v) => set('searchInMailbox', v)}
          options={[
            { value: 'on', label: t('search-highlight-on') },
            { value: 'off', label: t('search-highlight-off') },
          ]}
        />
      </section>

      <section className="field">
        <div className="flabel">{t('search-chips')}</div>
        <Pill
          label={t('search-chips')}
          value={settings.searchChips}
          onChange={(v) => set('searchChips', v)}
          options={[
            { value: 'on', label: t('search-highlight-on') },
            { value: 'off', label: t('search-highlight-off') },
          ]}
        />
      </section>

      <section className="field last">
        <div className="flabel">{t('appearance-search-highlight')}</div>
        <Pill
          label={t('appearance-search-highlight')}
          value={settings.searchHighlight}
          onChange={(v) => set('searchHighlight', v)}
          options={[
            { value: 'on', label: t('search-highlight-on') },
            { value: 'off', label: t('search-highlight-off') },
          ]}
        />
      </section>
    </div>
  );
}
