import { Monitor, Moon, Sun, type LucideIcon } from 'lucide-react';
import { resolveLocale, DEFAULTS, useSettings, type Settings } from '../../lib/settings';
import { Icon } from '../Icon';
import { availableLocales, t, type StringId } from '../../lib/strings';

const ACCENTS = ['#0E7C86', '#3B6EA5', '#6B5CA5', '#9A6B1F', '#5E7C4A', '#A8544B'];

const THEMES: { value: Settings['theme']; label: StringId; icon: LucideIcon }[] = [
  { value: 'light', label: 'theme-light', icon: Sun },
  { value: 'dark', label: 'theme-dark', icon: Moon },
  { value: 'system', label: 'theme-system', icon: Monitor },
];

/** A segmented control: few options, all worth showing at once. */
function Pill<T extends string>({
  value, options, onChange,
}: {
  value: T;
  options: { value: T; label: string }[];
  onChange: (v: T) => void;
}) {
  return (
    <div className="pill" role="group">
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          className={o.value === value ? 'on' : undefined}
          aria-pressed={o.value === value}
          onClick={() => onChange(o.value)}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

export function Appearance() {
  const { settings, set } = useSettings();

  return (
    <div className="pane-body">
      <h1 className="pane-title">{t('settings-appearance')}</h1>

      <section className="field">
        <div className="flabel">{t('appearance-theme')}</div>
        <p className="fhelp">{t('appearance-theme-help')}</p>
        <div className="seg" role="group">
          {THEMES.map((th) => (
            <button
              key={th.value}
              type="button"
              className={settings.theme === th.value ? 'on' : undefined}
              aria-pressed={settings.theme === th.value}
              onClick={() => set('theme', th.value)}
            >
              <Icon icon={th.icon} size={19} />
              {t(th.label)}
            </button>
          ))}
        </div>
      </section>

      <section className="field">
        <div className="flabel">{t('appearance-language')}</div>
        <p className="fhelp">{t('appearance-language-help')}</p>
        <select
          className="select"
          value={settings.language}
          onChange={(e) => set('language', e.target.value)}
        >
          {/* Enumerated from the bundles that exist rather than a hand-kept
              list, so adding a locale really is a data change. Each is named in
              its own language: someone looking for German is looking for
              "Deutsch", not for the English word for it. */}
          <option value="system">{t('language-system', { language: endonym(resolveLocale('system')) })}</option>
          {availableLocales().map((code) => (
            <option key={code} value={code}>
              {endonym(code)}
            </option>
          ))}
        </select>
      </section>

      <section className="field">
        <div className="flabel">{t('appearance-accent')}</div>
        <p className="fhelp">{t('appearance-accent-help')}</p>
        <div className="dotrow">
          {ACCENTS.map((c) => (
            <button
              key={c}
              type="button"
              className={`acc${settings.accent === c ? ' on' : ''}`}
              style={{ background: c }}
              aria-label={c}
              aria-pressed={settings.accent === c}
              onClick={() => set('accent', c)}
            />
          ))}
        </div>
      </section>

      <section className="field">
        <div className="flabel">{t('appearance-list')}</div>
        <p className="fhelp">{t('appearance-list-help')}</p>
        <div className="sub-controls">
          <div>
            <div className="sublabel">{t('appearance-density')}</div>
            <Pill
              value={settings.density}
              onChange={(v) => set('density', v)}
              options={[
                { value: 'relaxed', label: t('density-relaxed') },
                { value: 'compact', label: t('density-compact') },
              ]}
            />
          </div>
          <div>
            <div className="sublabel">{t('appearance-reading-pane')}</div>
            <Pill
              value={settings.layout}
              onChange={(v) => set('layout', v)}
              options={[
                { value: 'right', label: t('layout-right') },
                { value: 'below', label: t('layout-below') },
                { value: 'off', label: t('layout-off') },
              ]}
            />
          </div>
        </div>
      </section>

      {/* Here rather than under Notifications, where this started. "Badges"
          is grouped with notifications in the spec, but that means the dock
          icon — an interruption. These are numbers in the sidebar: a question
          about how the app looks, and Appearance is where people look. */}
      <section className="field">
        <div className="flabel">{t('badges')}</div>
        <p className="fhelp">{t('badges-help')}</p>
        <Pill
          value={settings.badges}
          onChange={(v) => set('badges', v)}
          options={[
            { value: 'unread', label: t('badges-unread') },
            { value: 'total', label: t('badges-total') },
            { value: 'off', label: t('badges-off') },
          ]}
        />
      </section>

      <section className="field">
        <div className="flabel">{t('appearance-checkboxes')}</div>
        <p className="fhelp">{t('appearance-checkboxes-help')}</p>
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
        <div className="flabel">{t('appearance-text-size')}</div>
        <p className="fhelp">{t('appearance-text-size-help')}</p>
        <div className="slider-row">
          <span className="slider-a small">A</span>
          <input
            type="range"
            min={12}
            max={20}
            step={1}
            value={Number(settings.readingTextSize)}
            onChange={(e) => set('readingTextSize', e.target.value)}
            aria-label={t('appearance-text-size')}
          />
          <span className="slider-a large">A</span>
          <span className="mono slider-value">{settings.readingTextSize}px</span>
          {settings.readingTextSize !== DEFAULTS.readingTextSize && (
            <button type="button" className="link-btn" onClick={() => set('readingTextSize', DEFAULTS.readingTextSize)}>
              {t('reset')}
            </button>
          )}
        </div>
      </section>
    </div>
  );
}

/** A language's name in that language. Intl knows them; a hand-kept table would
 *  go stale the moment a locale is added, which is the thing this file is
 *  trying to stop being. */
function endonym(code: string): string {
  try {
    return new Intl.DisplayNames([code], { type: 'language' }).of(code) ?? code;
  } catch {
    return code;
  }
}
