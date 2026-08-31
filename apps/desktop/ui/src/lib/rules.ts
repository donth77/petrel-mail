import type { StringId } from './strings';

/** What a condition looks at. */
export type RuleField =
  | 'from' | 'to' | 'cc' | 'subject' | 'body' | 'list_id' | 'header' | 'size' | 'date';

/** How it compares. */
export type RuleOp =
  | 'contains' | 'not_contains'
  | 'is' | 'is_not'
  | 'starts_with' | 'not_starts_with'
  | 'ends_with' | 'not_ends_with'
  | 'over' | 'under'
  | 'before' | 'after';

export const RULE_FIELDS: RuleField[] = [
  'from', 'to', 'cc', 'subject', 'body', 'list_id', 'header', 'size', 'date',
];

const TEXT_OPS: RuleOp[] = [
  'contains', 'not_contains',
  'is', 'is_not',
  'starts_with', 'not_starts_with',
  'ends_with', 'not_ends_with',
];

/**
 * The operators a field can take.
 *
 * Not one list for everything, because most pairings are nonsense: a subject
 * is never "over" anything and a size never "starts with". Offering them
 * anyway would mean a rule that can be built in the editor and then silently
 * never fires, which is the worst kind of setting — it looks configured.
 */
export function opsFor(field: RuleField): RuleOp[] {
  if (field === 'size') return ['over', 'under'];
  if (field === 'date') return ['before', 'after'];
  return TEXT_OPS;
}

/** What the value box should be: free text, a number of kilobytes, or a day. */
export function valueKind(field: RuleField): 'text' | 'number' | 'date' {
  if (field === 'size') return 'number';
  if (field === 'date') return 'date';
  return 'text';
}

/**
 * The operator to fall back to when the field changes under a condition.
 *
 * Switching From to Size leaves "contains" selected against a field that has
 * no such test, and a rule carrying that pairing matches nothing at all. The
 * editor moves it to the first operator the new field does offer.
 */
export function opForField(field: RuleField, current: RuleOp): RuleOp {
  const allowed = opsFor(field);
  return allowed.includes(current) ? current : allowed[0];
}

export const FIELD_LABEL: Record<RuleField, StringId> = {
  from: 'rule-field-from',
  to: 'rule-field-to',
  cc: 'rule-field-cc',
  subject: 'rule-field-subject',
  body: 'rule-field-body',
  list_id: 'rule-field-list_id',
  header: 'rule-field-header',
  size: 'rule-field-size',
  date: 'rule-field-date',
};

export const OP_LABEL: Record<RuleOp, StringId> = {
  contains: 'rule-op-contains',
  not_contains: 'rule-op-not-contains',
  is: 'rule-op-is',
  is_not: 'rule-op-is-not',
  starts_with: 'rule-op-starts-with',
  not_starts_with: 'rule-op-not-starts-with',
  ends_with: 'rule-op-ends-with',
  not_ends_with: 'rule-op-not-ends-with',
  over: 'rule-op-over',
  under: 'rule-op-under',
  before: 'rule-op-before',
  after: 'rule-op-after',
};
