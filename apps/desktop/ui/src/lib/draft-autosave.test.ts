import { describe, expect, it } from 'vitest';
import { draftHasContent, draftSignature, slotFor, unsaved } from './draft-autosave';

const blank = { to: '', cc: '', subject: '', body: '', html: '' };

describe('draftHasContent', () => {
  it('is false for an untouched composer', () => {
    expect(draftHasContent(blank)).toBe(false);
    expect(draftHasContent({ ...blank, body: '   \n' })).toBe(false);
  });

  it('counts a Cc line or an attachment as content', () => {
    expect(draftHasContent({ ...blank, cc: 'a@example.com' })).toBe(true);
    expect(
      draftHasContent({ ...blank, attachments: [{ path: '/tmp/a.pdf', name: 'a.pdf', size: 1 }] }),
    ).toBe(true);
  });
});

describe('draftSignature', () => {
  it('changes with what would be written and not with the saved id', () => {
    const a = { ...blank, body: 'hello', savedId: null };
    expect(draftSignature(a)).toBe(draftSignature({ ...a, savedId: 42 }));
    expect(draftSignature(a)).not.toBe(draftSignature({ ...a, body: 'hello!' }));
    expect(draftSignature(a)).not.toBe(
      draftSignature({ ...a, attachments: [{ path: '/x', name: 'x', size: 0 }] }),
    );
  });
});

describe('unsaved', () => {
  const signed = { ...blank, body: '\n\n-- \nTom', html: '<p>-- </p><p>Tom</p>' };

  it('is false for a fresh message that holds nothing but its signature', () => {
    const slot = slotFor(signed);
    expect(slot.id).toBeNull();
    expect(unsaved(signed, slot)).toBe(false);
  });

  it('is true once something is typed, and false again after that is saved', () => {
    const slot = slotFor(signed);
    const typed = { ...signed, subject: 'lunch' };
    expect(unsaved(typed, slot)).toBe(true);
    slot.id = 7;
    slot.signature = draftSignature(typed);
    expect(unsaved(typed, slot)).toBe(false);
    expect(unsaved({ ...typed, savedId: 7 }, slot)).toBe(false);
    expect(unsaved({ ...typed, subject: 'lunch?' }, slot)).toBe(true);
  });

  it('treats a draft from the store as matching its stored copy', () => {
    const loaded = { ...blank, to: 'sam@example.com', body: 'first words', savedId: 12 };
    const slot = slotFor(loaded);
    expect(slot.id).toBe(12);
    expect(unsaved(loaded, slot)).toBe(false);
    expect(unsaved({ ...loaded, body: 'first words, then more' }, slot)).toBe(true);
  });

  it('never counts an emptied message as worth writing', () => {
    const slot = slotFor({ ...blank, subject: 'x', savedId: 3 });
    expect(unsaved(blank, slot)).toBe(false);
  });
});
