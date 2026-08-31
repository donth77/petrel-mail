import { describe, expect, it } from 'vitest';
import { repaintTag } from './tag-paint';

type Row = { id: number; tags: { id: number; name: string; colour: string }[] };

const rows = (): Row[] => [
  { id: 1, tags: [{ id: 11, name: 'Urgent', colour: '#A8544B' }] },
  { id: 2, tags: [] },
  {
    id: 3,
    tags: [
      { id: 11, name: 'Urgent', colour: '#A8544B' },
      { id: 12, name: 'Waiting on', colour: '#3B6EA5' },
    ],
  },
];

describe('repainting a tag on the rows that wear it', () => {
  it('recolours every row carrying the tag', () => {
    const out = repaintTag(rows(), 11, '#6b46c1');
    expect(out[0].tags[0].colour).toBe('#6b46c1');
    expect(out[2].tags[0].colour).toBe('#6b46c1');
  });

  it('leaves the other tags on a row alone', () => {
    const out = repaintTag(rows(), 11, '#6b46c1');
    expect(out[2].tags[1]).toEqual({ id: 12, name: 'Waiting on', colour: '#3B6EA5' });
  });

  it('keeps the name, which a rename may be changing at the same moment', () => {
    const out = repaintTag(rows(), 11, '#6b46c1');
    expect(out[0].tags[0].name).toBe('Urgent');
  });

  it('does not touch a row that does not carry the tag', () => {
    const before = rows();
    const out = repaintTag(before, 11, '#6b46c1');
    expect(out[1]).toBe(before[1]);
  });

  it('hands back the same list when no row carries the tag', () => {
    const before = rows();
    expect(repaintTag(before, 99, '#6b46c1')).toBe(before);
  });

  it('does not mutate what it was given', () => {
    const before = rows();
    repaintTag(before, 11, '#6b46c1');
    expect(before[0].tags[0].colour).toBe('#A8544B');
  });

  it('matches on id, not on name, so two tags named alike do not both move', () => {
    const alike: Row[] = [
      { id: 1, tags: [{ id: 11, name: 'Urgent', colour: '#A8544B' }] },
      { id: 2, tags: [{ id: 22, name: 'Urgent', colour: '#3B6EA5' }] },
    ];
    const out = repaintTag(alike, 11, '#6b46c1');
    expect(out[0].tags[0].colour).toBe('#6b46c1');
    expect(out[1].tags[0].colour).toBe('#3B6EA5');
  });
});
