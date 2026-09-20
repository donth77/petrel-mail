import { describe, expect, it } from 'vitest';
import {
  ESSENTIAL,
  sectionOf,
  SECTIONS,
  isHidden,
  movedSection,
  readSections,
  shippedSections,
  toggledSection,
  visibleSections,
  writeSections,
} from './rail-sections';

describe('the sections a rail draws', () => {
  it('ships as places, labels, then questions', () => {
    expect(visibleSections(shippedSections())).toEqual([
      'mailboxes',
      'folders',
      'tags',
      'searches',
    ]);
  });

  it('reads back what was written', () => {
    const arranged = movedSection(toggledSection(shippedSections(), 'folders'), 'searches', true);
    expect(readSections(writeSections(arranged))).toEqual(arranged);
  });

  /* A stored setting is text that a later version, an earlier version, or a
     careless hand may have written. None of those may leave a rail with a
     group missing. */
  it('survives a setting that says something else entirely', () => {
    for (const said of ['', 'not json', 'null', '[]', '{}', '{"order":5}', '{"order":["moon"]}']) {
      expect(visibleSections(readSections(said)), said).toEqual(SECTIONS);
    }
  });

  it('puts back a section the stored order left out', () => {
    // A store written before searches existed, or by a version that dropped
    // one: the rest keep their arrangement and the absentee returns.
    const older = JSON.stringify({ order: ['tags', 'mailboxes'], hidden: [] });
    expect(visibleSections(readSections(older))).toEqual([
      'tags',
      'mailboxes',
      'folders',
      'searches',
    ]);
  });

  it('never hides the one that holds the inbox', () => {
    expect(isHidden(toggledSection(shippedSections(), ESSENTIAL), ESSENTIAL)).toBe(false);
    // Nor when a stored setting claims it is hidden.
    const said = JSON.stringify({ order: SECTIONS, hidden: ['mailboxes', 'tags'] });
    expect(visibleSections(readSections(said))).toEqual(['mailboxes', 'folders', 'searches']);
    // And it is not written back out as hidden either.
    expect(writeSections({ order: SECTIONS, hidden: ['mailboxes'] })).not.toContain('mailboxes"]');
  });

  it('hides one and shows it again', () => {
    const without = toggledSection(shippedSections(), 'tags');
    expect(visibleSections(without)).toEqual(['mailboxes', 'folders', 'searches']);
    expect(isHidden(without, 'tags')).toBe(true);
    expect(visibleSections(toggledSection(without, 'tags'))).toEqual(SECTIONS);
  });

  /* Hiding keeps the order, so turning a section off and on again puts it back
     where it was rather than at the end. */
  it('remembers where a hidden section belongs', () => {
    const moved = movedSection(shippedSections(), 'searches', true);
    expect(moved.order).toEqual(['mailboxes', 'folders', 'searches', 'tags']);
    const hidden = toggledSection(moved, 'searches');
    expect(visibleSections(toggledSection(hidden, 'searches'))).toEqual(moved.order);
  });

  describe('moving one', () => {
    it('swaps with its neighbour', () => {
      expect(movedSection(shippedSections(), 'tags', true).order).toEqual([
        'mailboxes',
        'tags',
        'folders',
        'searches',
      ]);
      expect(movedSection(shippedSections(), 'mailboxes', false).order).toEqual([
        'folders',
        'mailboxes',
        'tags',
        'searches',
      ]);
    });

    it('does nothing at either end, so the button can stay put', () => {
      const s = shippedSections();
      expect(movedSection(s, 'mailboxes', true).order).toEqual(s.order);
      expect(movedSection(s, 'searches', false).order).toEqual(s.order);
    });
  });
});

describe('which section a view belongs to', () => {
  it('places the views that name their own kind', () => {
    expect(sectionOf('tag:Urgent')).toBe('tags');
    expect(sectionOf('folder:7')).toBe('folders');
    expect(sectionOf('search:3')).toBe('searches');
  });

  /* Everything else is a mailbox, which is where the fallback lives — including
     a view key from a version this one does not know. */
  it('treats everything else as a mailbox', () => {
    for (const view of ['inbox', 'sent', 'starred', 'snoozed', 'outbox', '', 'moon']) {
      expect(sectionOf(view), view).toBe('mailboxes');
    }
  });

  /* Hiding the section you are standing in must not leave the window showing a
     collection with no way back to it. */
  it('says when the view on screen has just been hidden', () => {
    const hidden = toggledSection(shippedSections(), 'tags');
    const shown = visibleSections(hidden);
    expect(shown.includes(sectionOf('tag:Urgent'))).toBe(false);
    expect(shown.includes(sectionOf('inbox'))).toBe(true);
  });
});
