import { createContext, useCallback, useContext, useEffect, useMemo, useState } from 'react';
import { api } from './api';
import { setFormatPrefs, type ClockPref } from './format';

/**
 * Preferences, with their defaults in one place.
 *
 * A default is *absent* from storage, not written into it — so if a default
 * later changes, everyone who never chose otherwise moves with it, rather than
 * being silently pinned to the old value by a row nobody knew was there.
 */
export const DEFAULTS = {
  theme: 'system' as 'system' | 'light' | 'dark',
  accent: '#0E7C86',
  density: 'relaxed' as 'relaxed' | 'compact',
  layout: 'right' as 'right' | 'below' | 'off',
  readingTextSize: '15',
  language: 'system',
  clock: 'system' as ClockPref,

  // Notifications (docs/design SettingsNotifications).
  //
  // What earns an interruption, rather than a bare on/off: "all new mail" is
  // the setting people turn off entirely after a week, and turning it off is
  // how you stop hearing about the one message that mattered.
  notifyLevel: 'all' as 'all' | 'priority' | 'none',
  /// Desktop notifications go through the OS, which can refuse them. The in-app
  /// toast is separate and always available, so this only governs the OS ones.
  notifyDesktop: 'on' as 'on' | 'off',
  /// A timestamp in ms; notifications stay silent until it passes. Stored as an
  /// instant rather than a boolean so a pause cannot outlive its own intent by
  /// being forgotten in the off position.
  notifyPausedUntil: '0',

  /// Seconds to hold a message before it goes. Nothing reaches the server while
  /// the countdown runs, which is what makes undo a cancel rather than a recall
  /// — the only kind that actually works (docs/design Compose).
  undoSendSeconds: '10',
};

export type Settings = typeof DEFAULTS;
type Key = keyof Settings;

type Ctx = {
  settings: Settings;
  set: <K extends Key>(key: K, value: Settings[K]) => void;
  reset: (key: Key) => void;
};

const SettingsContext = createContext<Ctx | null>(null);

export function SettingsProvider({ children }: { children: React.ReactNode }) {
  const [stored, setStored] = useState<Record<string, string>>({});

  useEffect(() => {
    let live = true;
    api
      .getSettings()
      .then((s) => live && setStored(s))
      .catch((err) => api.log(`get_settings failed: ${err}`));
    return () => {
      live = false;
    };
  }, []);

  const settings = useMemo(() => {
    const merged = { ...DEFAULTS };
    for (const k of Object.keys(DEFAULTS) as Key[]) {
      const v = stored[k];
      if (v !== undefined && v !== '') (merged as Record<string, string>)[k] = v;
    }
    return merged;
  }, [stored]);

  // Applied where the platform, not React, does the work: the theme attribute
  // drives the token blocks, and Intl formatters are rebuilt in one place.
  useEffect(() => {
    const root = document.documentElement;
    if (settings.theme === 'system') root.removeAttribute('data-theme');
    else root.setAttribute('data-theme', settings.theme);
    root.style.setProperty('--accent-user', settings.accent);
    root.style.setProperty('--reading-size', `${settings.readingTextSize}px`);
  }, [settings.theme, settings.accent, settings.readingTextSize]);

  useEffect(() => {
    setFormatPrefs({
      clock: settings.clock,
      locale: settings.language === 'system' ? undefined : settings.language,
    });
  }, [settings.clock, settings.language]);

  const set = useCallback(<K extends Key>(key: K, value: Settings[K]) => {
    // Optimistic: a preference that lags behind the control feels broken, and
    // the write is local and effectively instant.
    setStored((s) => ({ ...s, [key]: String(value) }));
    api.setSetting(key, String(value)).catch((err) => api.log(`set_setting ${key}: ${err}`));
  }, []);

  const reset = useCallback((key: Key) => {
    setStored((s) => {
      const next = { ...s };
      delete next[key];
      return next;
    });
    api.setSetting(key, '').catch(() => {});
  }, []);

  return (
    <SettingsContext.Provider value={{ settings, set, reset }}>{children}</SettingsContext.Provider>
  );
}

export function useSettings(): Ctx {
  const ctx = useContext(SettingsContext);
  if (!ctx) throw new Error('useSettings outside SettingsProvider');
  return ctx;
}
