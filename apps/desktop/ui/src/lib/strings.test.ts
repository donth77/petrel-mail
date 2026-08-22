import { describe, expect, it } from 'vitest';
import { en, t } from './strings';

describe('interpolation', () => {
  /* A placeholder in the wrong shape fails silently: t() leaves it alone and
     the literal "{subject}" is rendered to the user. Nothing type-checks it,
     nothing throws, and it is only ever caught by someone reading the screen.
     Three strings shipped that way before this test existed. */
  it('every placeholder uses the form t() actually substitutes', () => {
    const wrong: string[] = [];
    for (const [id, text] of Object.entries(en)) {
      for (const m of String(text).matchAll(/\{([^}]*)\}/g)) {
        if (!/^\s*\$\w+\s*$/.test(m[1])) wrong.push(`${id}: ${m[0]}`);
      }
    }
    expect(wrong).toEqual([]);
  });

  it('substitutes, and leaves an unknown placeholder legible', () => {
    expect(t('delete-forever-one', { subject: 'Q3 budget' })).toContain('Q3 budget');
    expect(t('delete-forever-one', {})).toContain('$subject');
  });
});
