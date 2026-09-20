import { createContext, useCallback, useContext, useEffect, useMemo, useState } from 'react';
import { api } from './api';
import { setFormatPrefs, type ClockPref } from './format';
import { availableLocales, setLocale } from './strings';

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

  // Notifications.
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

  /// The numbers beside the rail's mailboxes.
  ///
  /// Unread by default, because the question a mailbox has to answer is
  /// usually "is there anything here for me". Total is for anyone who wants
  /// the rail to say how big each mailbox is; off is for anyone who would
  /// rather not be counted at.
  badges: 'unread' as 'unread' | 'total' | 'off',

  /// How the sidebar's mailboxes are arranged: their order, which of them are
  /// shown, and what number each one carries. JSON, written by the Sidebar
  /// pane and read by lib/mailboxes.ts, which stores only what differs from
  /// the defaults so an improved default still reaches anyone who never
  /// overrode that row. Empty until somebody changes something, which is when
  /// `badges` above stops being consulted.
  railMailboxes: '',
  /** Days a message may sit in the Trash before Petrel deletes it, on the
   *  server and here. '0' is off, and is the default: deleting mail on a
   *  timer is a promise to opt into rather than a default to discover. */
  trashRetentionDays: '0' as '0' | '7' | '30' | '90',

  /// A checkbox column down the left of the list.
  ///
  /// Off by default: the avatar already selects, which costs no width, and a
  /// permanent column of empty boxes is space every row pays for all the time
  /// to serve the minority of moments anyone is selecting. On for people who
  /// expect it from every other mail client, and for whom an avatar that is
  /// secretly a checkbox is a thing you have to be told.
  checkboxes: 'off' as 'off' | 'on',

  /// Mark a search's words where they were found: in the list's subjects and
  /// previews, and in the message once it is open.
  ///
  /// On by default, because a result that does not show why it is there reads
  /// as a wrong result. Off for anyone who finds a page of yellow harder to
  /// read than a page without it, which on a common word it can be.
  searchHighlight: 'on' as 'on' | 'off',

  /// Start a search in the mailbox you are looking at, by writing its scope
  /// into the empty field: `in:inbox`, `in:receipts`.
  ///
  /// On, because a search begun in a folder is nearly always a search of that
  /// folder, and the token is there to be deleted the moment it is not. Off
  /// for anyone who searches everything and tires of deleting it.
  searchInMailbox: 'on' as 'on' | 'off',

  /// Show the filter buttons above the list while a search is being typed.
  ///
  /// On, because they are how the grammar is discovered: each one writes what
  /// it means into the field, where it can be read and edited. Off for anyone
  /// who knows the grammar and would rather have the row back.
  searchChips: 'on' as 'on' | 'off',

  /// How the list is ordered, and how search results are, remembered as
  /// `key:direction`. Two, because they are two questions: a mailbox cannot
  /// be ordered by relevance and a search usually should be.
  ///
  /// `listSort` is the order everything takes until a mailbox is given one
  /// of its own; `listSortByView` is those, one entry to a view, kept even
  /// while the shared order is in force so that turning it off restores
  /// them. Mail, Outlook, Thunderbird and the Finder all remember per
  /// folder, and the reason is that folders differ: Sent is a list of people
  /// you wrote to and reads well by name, while an inbox almost never does.
  listSort: 'date:descending',
  listSortByView: '{}',
  /// Whether a mailbox keeps the order you give it, or every list shares
  /// one. Per mailbox by default, as Mail, Outlook, Thunderbird and the
  /// Finder all do, because folders differ: Sent is a list of people you
  /// wrote to and reads well by name, while an inbox almost never does.
  ///
  /// A setting rather than a *use this everywhere* item in the menu, which
  /// was the first try: that applied one order once, and the next sort
  /// anywhere wrote a mailbox's own again, so somebody who wanted one order
  /// for everything had to keep saying so. A policy is said once.
  sortScope: 'mailbox' as 'mailbox' | 'everywhere',
  searchSort: 'relevance:descending',

  /// Seconds to hold a message before it goes. Nothing reaches the server while
  /// the countdown runs, which is what makes undo a cancel rather than a recall
  /// — the only kind that actually works.
  undoSendSeconds: '10',
  /// Warn before sending a message that mentions an attachment and has none.
  warnMissingAttachment: 'on' as 'on' | 'off',
  /// Which button the R key and the reply row lead with.
  replyDefault: 'reply' as 'reply' | 'reply-all',

  /// Sidebar width in pixels, and whether it is collapsed to icons. Stored as
  /// strings like every other setting so the persistence layer stays one shape.
  /// Remote images and other external resources in message bodies. On by
  /// default because loading one tells the sender the message was opened, by
  /// whom, and when — the default has to be the private one.
  blockRemoteContent: 'on' as 'on' | 'off',

  railWidth: '236',
  listWidth: '430',
  railCollapsed: 'off' as 'on' | 'off',
};

/** Width of the collapsed rail: one icon plus its hit area, nothing else. */
export const RAIL_COLLAPSED = 56;
export const RAIL_MIN = 180;
export const RAIL_MAX = 380;

/* The conversation list's width. Its floor is a readable row rather than an
   arbitrary number: below about 300px the sender, the time and the subject stop
   fitting on the lines they are meant to share. Its ceiling leaves the reading
   pane its own `minmax(380px, 1fr)`, so dragging can crowd the reader but never
   squeeze it out. */
export const LIST_MIN = 300;
export const LIST_MAX = 720;

/** Keeps a stored width usable. A rail dragged to 12px, or corrupted to NaN by
 *  a hand-edited settings row, would otherwise be unrecoverable without
 *  clearing settings — the handle would be too small to grab. */
export function clampRail(value: string | number): number {
  // Empty and whitespace are "no value", not zero. Number('') is 0, so without
  // this an absent width clamps to the minimum and the sidebar silently comes
  // back at its narrowest instead of the width it is supposed to default to.
  if (typeof value === 'string' && value.trim() === '') return Number(DEFAULTS.railWidth);
  const n = typeof value === 'number' ? value : Number(value);
  if (!Number.isFinite(n)) return Number(DEFAULTS.railWidth);
  return Math.min(RAIL_MAX, Math.max(RAIL_MIN, Math.round(n)));
}

/** The conversation list's width, kept inside its bounds. Same shape as
    `clampRail`, and separate from it because the two have different floors and
    a shared clamp would give one of them the other's. */
export function clampList(value: string | number): number {
  if (typeof value === 'string' && value.trim() === '') return Number(DEFAULTS.listWidth);
  const n = typeof value === 'number' ? value : Number(value);
  if (!Number.isFinite(n)) return Number(DEFAULTS.listWidth);
  return Math.min(LIST_MAX, Math.max(LIST_MIN, Math.round(n)));
}

export type Settings = typeof DEFAULTS;
type Key = keyof Settings;

type Ctx = {
  settings: Settings;
  /** The locale actually in use, which is not settings.language: that may say
   *  `system`. Anything that caches a translated value keys its cache on this,
   *  because t() is a plain function and a useMemo has no other way to know
   *  the words underneath it changed. */
  locale: string;
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

  // Which language the interface speaks. "system" follows the Mac, but only as
  // far as a locale we actually ship: asking for de-AT when only de exists
  // should give German, and asking for something we have nothing for should
  // give English rather than a screen of ids.
  const resolved = resolveLocale(settings.language);

  // Applied where the platform, not React, does the work: the theme attribute
  // drives the token blocks, and Intl formatters are rebuilt in one place.
  useEffect(() => {
    const root = document.documentElement;
    if (settings.theme === 'system') root.removeAttribute('data-theme');
    else root.setAttribute('data-theme', settings.theme);
    // What language the chrome is in, for the platform rather than for us.
    //
    // index.html ships `lang="en"` and nothing used to move it. WebKit — which
    // is what the app runs in — picks the font for a Han character from this
    // attribute, and Japanese, Simplified Chinese and Traditional Chinese draw
    // several of the same code points differently. Left at English it resolves
    // to the Japanese face, so a Simplified Chinese reader saw Japanese glyph
    // forms. (Chromium ignores the attribute here, so the harness cannot see
    // this and the real engine has to be asked.) VoiceOver reads from it too:
    // a Japanese interface was being spoken in an English voice.
    root.lang = resolved;
    // Our faces, or the platform's. Set as inline style on the same element the
    // tokens are declared on, so it wins without a second rule to keep in step.
    if (usesSystemType(resolved)) {
      root.style.setProperty('--body', SYSTEM_TYPE);
      root.style.setProperty('--display', SYSTEM_TYPE);
    } else {
      root.style.removeProperty('--body');
      root.style.removeProperty('--display');
    }
    root.style.setProperty('--accent-user', settings.accent);
    root.style.setProperty('--reading-size', `${settings.readingTextSize}px`);
    // The rail's width is a token so the three-pane grid picks it up without
    // the layout needing to know a drag happened. Its floor travels with it:
    // the grid holds the rail at a minimum so the reader is not pushed off
    // the right edge, and a floor of 180px against a collapsed rail of 56px
    // is a rail that does not collapse — a minimum larger than the size
    // asked for is the width the track ends up at.
    const collapsed = settings.railCollapsed === 'on';
    root.style.setProperty(
      '--rail-size',
      collapsed ? `${RAIL_COLLAPSED}px` : `${clampRail(settings.railWidth)}px`,
    );
    root.style.setProperty('--rail-min', collapsed ? `${RAIL_COLLAPSED}px` : `${RAIL_MIN}px`);
    root.style.setProperty('--list-size', `${clampList(settings.listWidth)}px`);
    // Depends on the whole object, not a hand-listed subset. `settings` is
    // memoised on `stored`, so this runs exactly when a preference changes —
    // and adding a line to the body can no longer silently do nothing because
    // its key was left out of the list, which is precisely what happened when
    // the rail width was added here.
  }, [settings, resolved]);

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

  // Set during render, not in an effect. An effect runs after the children have
  // already rendered, so the first paint after a language change would still be
  // in the old language. This is a module-level assignment, cheap and
  // idempotent, so running it every render costs nothing.
  setLocale(resolved);

  return (
    <SettingsContext.Provider value={{ settings, locale: resolved, set, reset }}>
      {/* Not keyed on the language any more.
          It used to be: `<Fragment key={resolved}>`, so changing the language
          remounted the whole tree and every component re-ran t(). It also
          threw away all React state below it — including which Settings pane
          was open, so choosing a language closed the window you chose it in.

          A remount was never needed to do this. The provider's value is a new
          object on every render, so every useSettings() consumer re-renders
          when the language changes, and App is one — which re-renders
          everything under it, since nothing here is memo()'d. What a re-render
          does NOT refresh is a useMemo that caches translated text, and those
          now take `locale` as a dependency. help.test.tsx holds that line. */}
      {children}
    </SettingsContext.Provider>
  );
}

/** Requested language to one we ship: exact match, then base language, then
 *  English. `system` asks the browser first.
 *
 *  Exported so the Appearance pane can say which language "System" actually
 *  resolves to. It used to name that in the string itself — every bundle
 *  hardcoded its own language, so the English bundle said "System (English)"
 *  even on a Mac set to French, where System would have given French. Right by
 *  coincidence whenever the two agreed, and wrong the moment they did not. */
/** Which script a region writes Chinese in, for the regions that matter.
 *
 *  A platform reports `zh-CN`, not `zh-Hans-CN`, and the locale we ship is
 *  `zh-Hans`. Without this the Simplified Chinese translation could not be
 *  reached by "system" at all — `zh-CN` has no `zh` to fall back to, so it
 *  landed on English. Traditional regions are listed too, so they resolve to a
 *  `zh-Hant` we do not ship and then honestly give English rather than handing
 *  a Taiwanese reader Simplified characters. */
const CHINESE_SCRIPT: Record<string, string> = {
  CN: 'Hans',
  SG: 'Hans',
  MY: 'Hans',
  TW: 'Hant',
  HK: 'Hant',
  MO: 'Hant',
};

/** The tags to try for one platform language, best first.
 *
 *  Longest prefix down to the bare language, because a shipped locale can be
 *  more specific than the tag's base: `zh-Hans-CN` has to find `zh-Hans`, and
 *  stopping at `zh` finds nothing. */
function candidates(tag: string): string[] {
  const parts = tag.split('-').filter(Boolean);
  if (parts.length === 0) return [];
  const out: string[] = [];
  // A region-only Chinese tag gains the script its region writes.
  if (parts[0].toLowerCase() === 'zh' && parts.length >= 2) {
    const script = CHINESE_SCRIPT[parts[1].toUpperCase()];
    if (script) out.push(`zh-${script}`);
  }
  for (let i = parts.length; i > 0; i -= 1) out.push(parts.slice(0, i).join('-'));
  return out;
}

/** The stack the platform draws its own interface with. */
const SYSTEM_TYPE = '-apple-system, BlinkMacSystemFont, system-ui, sans-serif';

/** Whether the interface should be set in the platform's font rather than ours.
 *
 *  The two bundled faces cover Latin and Latin-ext only — that is what keeps
 *  them to 205KB and lets them ship offline — so a Japanese, Korean or Chinese
 *  interface draws its letters from the system stack whatever we do. What it
 *  used to do was draw them from *both*: Public Sans for the ASCII in a string
 *  and a system face for the CJK beside it, two typefaces of different weight
 *  and x-height on the same line. That is the unevenness issue 37 reported,
 *  though not the cause it proposed — the leading measured identical.
 *
 *  So for those locales the whole interface uses the platform's font, which is
 *  one typeface throughout and the one the rest of the machine is set in. The
 *  reading pane has always been on this stack, so the chrome now agrees with
 *  the message rather than differing from it. Latin locales keep the bundled
 *  faces, and nothing is downloaded either way. */
export function usesSystemType(locale: string): boolean {
  const base = locale.split('-')[0].toLowerCase();
  return base === 'ja' || base === 'ko' || base === 'zh';
}

export function resolveLocale(setting: string): string {
  const have = new Set(availableLocales());
  const wanted =
    setting && setting !== 'system'
      ? [setting]
      : typeof navigator !== 'undefined'
        ? [...(navigator.languages ?? []), navigator.language]
        : [];
  for (const tag of wanted) {
    if (!tag) continue;
    for (const candidate of candidates(tag)) {
      if (have.has(candidate)) return candidate;
    }
  }
  return 'en';
}

export function useSettings(): Ctx {
  const ctx = useContext(SettingsContext);
  if (!ctx) throw new Error('useSettings outside SettingsProvider');
  return ctx;
}
