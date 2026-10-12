import { afterEach, describe, expect, it } from 'vitest';
import type { ActionKind } from './api';
import { availableLocales, setLocale } from './strings';
import { doneText } from './useTriage';

const KINDS: ActionKind[] = [
  'archive',
  'trash',
  'spam',
  'star',
  'unstar',
  'mark_read',
  'mark_unread',
  'move',
  'tag',
  'untag',
  'snooze',
  'unsnooze',
  'delete_forever',
];

describe('what a triage toast says', () => {
  afterEach(() => setLocale('en'));

  it('is the reader’s language, not the engine’s English', () => {
    setLocale('de');
    expect(doneText('archive')).toBe('Archiviert');
    setLocale('ja');
    expect(doneText('trash')).toBe('ゴミ箱へ移動しました');
  });

  it('has words for every action in every language', () => {
    for (const locale of availableLocales()) {
      setLocale(locale);
      for (const kind of KINDS) {
        const said = doneText(kind);
        expect(said, `${locale} ${kind}`).not.toMatch(/^triage-done-/);
        expect(said.length, `${locale} ${kind}`).toBeGreaterThan(0);
      }
    }
  });
});
