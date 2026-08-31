import { describe, expect, it } from 'vitest';
import { FIELD_LABEL, OP_LABEL, RULE_FIELDS, opForField, opsFor, valueKind } from './rules';

/**
 * The editor's job is to make unmatchable rules untypeable.
 *
 * A rule is written once and then trusted for months, so the failure that
 * matters is not an error message — it is a rule that saves cleanly, sits in
 * the list looking enabled, and never fires. Every pairing the editor offers
 * has to be one the engine can actually evaluate.
 */
describe('rule fields and operators', () => {
  it('offers text operators for the text fields, both ways round', () => {
    for (const field of ['from', 'to', 'cc', 'subject', 'body', 'list_id', 'header'] as const) {
      const ops = opsFor(field);
      expect(ops).toContain('contains');
      expect(ops).toContain('not_contains');
      expect(ops).toContain('is');
      expect(ops).toContain('is_not');
      expect(ops).toContain('starts_with');
      expect(ops).toContain('not_starts_with');
      expect(ops).toContain('ends_with');
      expect(ops).toContain('not_ends_with');
    }
  });

  it('asks a number and a date their own questions', () => {
    // "Size contains 5" is not a thing, and offering it would be a rule that
    // saves and never matches.
    expect(opsFor('size')).toEqual(['over', 'under']);
    expect(opsFor('date')).toEqual(['before', 'after']);
    expect(opsFor('size')).not.toContain('contains');
    expect(opsFor('date')).not.toContain('contains');
  });

  it('moves the operator when the field stops accepting it', () => {
    // Switching From to Size with "contains" left selected is exactly how an
    // unmatchable rule gets saved.
    expect(opForField('size', 'contains')).toBe('over');
    expect(opForField('date', 'ends_with')).toBe('before');
    // And leaves it alone when it still applies.
    expect(opForField('subject', 'ends_with')).toBe('ends_with');
    expect(opForField('size', 'under')).toBe('under');
  });

  it('gives the value box the right kind', () => {
    expect(valueKind('size')).toBe('number');
    expect(valueKind('date')).toBe('date');
    expect(valueKind('from')).toBe('text');
  });

  it('names every field and every operator it offers', () => {
    // A missing label renders as a blank option, which is a menu entry that
    // cannot be chosen on purpose.
    for (const field of RULE_FIELDS) {
      expect(FIELD_LABEL[field]).toBeTruthy();
      for (const op of opsFor(field)) {
        expect(OP_LABEL[op]).toBeTruthy();
      }
    }
  });
});
