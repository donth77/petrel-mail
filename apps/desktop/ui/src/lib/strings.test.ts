import { describe, expect, it } from 'vitest';
import { FluentResource } from '@fluent/bundle';
import enFtl from '../locales/en.ftl?raw';
import { STRING_IDS } from './string-ids';
import { t } from './strings';

describe('the English bundle', () => {
  /* A syntax error in a .ftl is not loud. Fluent skips the entry it could not
     parse and carries on, so the only symptom is one string rendering as its
     own id, somewhere, later. */
  it('parses with no errors', () => {
    const resource = new FluentResource(enFtl);
    const junk = resource.body.filter((entry) => !('id' in entry));
    expect(junk).toEqual([]);
  });

  /* string-ids.ts is generated from the .ftl and gives call sites their
     compile-time check. If the two drift, the compiler starts vouching for
     ids that no longer exist, or refusing ones that do. */
  it('contains exactly the ids the generated list claims', () => {
    const inFile = new Set(
      enFtl
        .split('\n')
        .map((line) => /^([a-zA-Z][a-zA-Z0-9_-]*)\s*=/.exec(line)?.[1])
        .filter((id): id is string => Boolean(id)),
    );
    const generated = new Set<string>(STRING_IDS);
    const onlyInFtl = [...inFile].filter((id) => !generated.has(id));
    const onlyInList = [...generated].filter((id) => !inFile.has(id));
    expect({ onlyInFtl, onlyInList }).toEqual({ onlyInFtl: [], onlyInList: [] });
  });
});

describe('interpolation', () => {
  it('substitutes, and leaves an unknown placeholder legible', () => {
    expect(t('delete-forever-one', { subject: 'Q3 budget' })).toContain('Q3 budget');
    /* Not an exception. Fluent throws on a missing argument unless an errors
       array is passed, and t() passes one precisely so a forgotten
       interpolation shows up on screen instead of taking the pane down. */
    expect(t('delete-forever-one', {})).toContain('$subject');
  });

  it('counts with the plural form the number calls for', () => {
    expect(t('outbox-attachments', { count: 1 })).toBe('1 attachment');
    expect(t('outbox-attachments', { count: 2 })).toBe('2 attachments');
    expect(t('list-conversations', { count: 1 })).toBe('1 conversation');
    expect(t('list-conversations', { count: 7 })).toBe('7 conversations');
  });
});
