import { useSettings } from '../../lib/settings';
import { Pill } from './Pill';
import { t } from '../../lib/strings';

/**
 * The conversation list: how much of it you see at once, where the message
 * opens, how you pick rows, and what order they come in.
 *
 * Its own pane because none of it is quite Appearance, where it started.
 * Three of the four are about how the list looks, but the fourth is about
 * what order the mail is in, which is a fact about your mail rather than
 * about the window — and a pane called Appearance answering "why is Sent
 * ordered by name" is a pane whose name is wrong. Mail, Outlook and
 * Thunderbird all keep this group together and call it viewing or display.
 */
export function Lists() {
  const { settings, set } = useSettings();

  return (
    <div className="pane-body">
      <h1 className="pane-title">{t('settings-lists')}</h1>

      <section className="field">
        <div className="flabel">{t('appearance-density')}</div>
        <Pill
          value={settings.density}
          onChange={(v) => set('density', v)}
          options={[
            { value: 'relaxed', label: t('density-relaxed') },
            { value: 'compact', label: t('density-compact') },
          ]}
        />
      </section>

      <section className="field">
        <div className="flabel">{t('appearance-reading-pane')}</div>
        <Pill
          value={settings.layout}
          onChange={(v) => set('layout', v)}
          options={[
            { value: 'right', label: t('layout-right') },
            { value: 'below', label: t('layout-below') },
            { value: 'off', label: t('layout-off') },
          ]}
        />
      </section>

      <section className="field">
        <div className="flabel">{t('appearance-checkboxes')}</div>
        <Pill
          value={settings.checkboxes}
          onChange={(v) => set('checkboxes', v)}
          options={[
            { value: 'off', label: t('checkboxes-off') },
            { value: 'on', label: t('checkboxes-on') },
          ]}
        />
      </section>

      <section className="field last">
        <div className="flabel">{t('lists-sort-scope')}</div>
        <Pill
          value={settings.sortScope}
          onChange={(v) => set('sortScope', v)}
          options={[
            { value: 'mailbox', label: t('sort-scope-mailbox') },
            { value: 'everywhere', label: t('sort-scope-everywhere') },
          ]}
        />
      </section>
    </div>
  );
}
