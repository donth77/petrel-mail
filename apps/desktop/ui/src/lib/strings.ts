/* User-facing strings.
 *
 * Components never hold string literals (AGENTS.md). This module is the single
 * lookup they go through. The strings themselves live in `locales/*.ftl`, so
 * adding a language is a file plus a line in SOURCES, not a code change.
 *
 * Fluent rather than another record, for one reason above the rest: plurals.
 * "{ $count } conversations" is wrong in English at 1 and unfixable in Russian,
 * which has three forms, or Arabic, which has six. A ternary at the call site
 * cannot express that; a select expression in the .ftl can, and the translator
 * writes it rather than the programmer guessing. See docs 07 §13.
 */

import { FluentBundle, FluentResource } from '@fluent/bundle';
import enFtl from '../locales/en.ftl?raw';
import esFtl from '../locales/es.ftl?raw';
import frFtl from '../locales/fr.ftl?raw';
import deFtl from '../locales/de.ftl?raw';
import { type StringId } from './string-ids';

export { type StringId };

type Args = Record<string, string | number>;

/** Every locale that ships. One line and one file to add another. */
const SOURCES: Record<string, string> = {
  en: enFtl,
  es: esFtl,
  fr: frFtl,
  de: deFtl,
};

/** English is the floor. Nothing below it, so a missing translation shows the
 *  English rather than an id. */
const FALLBACK = 'en';

const bundles = new Map<string, FluentBundle>();

function bundleFor(locale: string): FluentBundle | null {
  const source = SOURCES[locale];
  if (source === undefined) return null;
  const cached = bundles.get(locale);
  if (cached) return cached;
  // useIsolating wraps every placeable in Unicode bidi marks. They matter when
  // a right-to-left string embeds a left-to-right name, and are invisible
  // otherwise — but they are real characters, so they reach the DOM, any
  // clipboard copy, and every assertion made about a string. Off until a
  // right-to-left locale ships, which is the moment to turn it back on.
  const bundle = new FluentBundle(locale, { useIsolating: false });
  bundle.addResource(new FluentResource(source));
  bundles.set(locale, bundle);
  return bundle;
}

/** Locale requested, then its base language, then English.
 *  `pt-BR` tries pt-BR, then pt, then en. */
function chain(locale: string): string[] {
  const want = [locale];
  const base = locale.split('-')[0];
  if (base && base !== locale) want.push(base);
  if (!want.includes(FALLBACK)) want.push(FALLBACK);
  return want;
}

let active = FALLBACK;

/** Which language the interface is in. Unknown or unshipped locales fall back
 *  rather than throwing, so a bad value in settings cannot brick the UI. */
export function setLocale(locale: string): void {
  active = locale || FALLBACK;
}

export function getLocale(): string {
  return active;
}

/** The locales that actually have a bundle, for the language picker. */
export function availableLocales(): string[] {
  return Object.keys(SOURCES);
}

export function t(id: StringId, args?: Args): string {
  for (const locale of chain(active)) {
    const bundle = bundleFor(locale);
    const message = bundle?.getMessage(id);
    if (!message?.value) continue;
    // The errors array is not optional in practice. Without it formatPattern
    // THROWS on a missing argument, so one forgotten interpolation would take
    // the whole pane down. With it, the placeholder source is rendered instead
    // — legible on screen, greppable in a bug report, and the same thing the
    // hand-rolled substitution used to do.
    const errors: Error[] = [];
    return bundle!.formatPattern(message.value, args, errors);
  }
  // Only reachable if an id is missing from English, which the id list and its
  // test exist to prevent. Showing the id beats showing nothing.
  return id;
}
