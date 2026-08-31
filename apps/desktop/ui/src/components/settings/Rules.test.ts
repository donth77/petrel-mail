import { describe, expect, it } from 'vitest';
import { summary } from './Rules';
import type { Folder, Rule, Tag } from '../../lib/api';

const folders = [{ id: 7, path: 'Marketing' }] as unknown as Folder[];
const tags = [{ id: 5, name: 'Invoices' }] as unknown as Tag[];

const rule = (actions: Partial<Rule['actions']>): Rule => ({
  id: 1,
  position: 0,
  enabled: true,
  name: 'r',
  conditions: [{ field: 'from', op: 'contains', value: 'dana@' }],
  actions: { move_to: null, tag: null, mark_read: false, skip_inbox: false, notify: false, ...actions },
});

/**
 * The line under a rule's name is the only place the rule explains itself, so
 * it has to describe what the engine will actually do — not what the tick
 * boxes say.
 */
describe('rule summary', () => {
  it('does not promise a skip the move already performs', () => {
    // planned_actions drops the archive when a destination is named: queueing
    // Move then Archive threw the move away, and filed the mail in Archive.
    const line = summary(rule({ move_to: 7, skip_inbox: true }), folders, tags);
    expect(line).toContain('Marketing');
    expect(line).not.toContain('skip inbox');
  });

  it('still says skip inbox when there is nowhere to go', () => {
    expect(summary(rule({ skip_inbox: true }), folders, tags)).toContain('skip inbox');
  });

  it('says a target is gone rather than printing a shrug', () => {
    // A rule outlives the folder it names. It used to read "move to ?", which
    // looks like a rendering glitch rather than a rule that cannot run.
    const gone = summary(rule({ move_to: 999, tag: 998 }), folders, tags);
    expect(gone).not.toContain('?');
    expect(gone).toContain('names something deleted');
  });

  it('reads out the actions a rule does carry', () => {
    const line = summary(rule({ tag: 5, mark_read: true, notify: true }), folders, tags);
    // The operator is named rather than left as a squiggle: "From contains"
    // and "From is exactly" are different rules and the line has to say which.
    expect(line).toBe('From contains “dana@” → tag Invoices, mark read, notify');
  });
});
