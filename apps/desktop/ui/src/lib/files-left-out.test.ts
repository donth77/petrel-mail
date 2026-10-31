import { describe, expect, it } from 'vitest';
import type { Draft } from '../components/Compose';
import { FilesLeftOut, keptWhole } from './files-left-out';

const written: Draft = {
  to: 'dana@example.com',
  cc: '',
  subject: 'Plans',
  body: 'See attached.',
  html: '',
  attachments: [],
  copying: [],
};
const chip = { id: 'drop-1', name: 'big.pdf', size: 4 };

describe('keptWhole', () => {
  it('stops an action that takes the message away when a file was left out', () => {
    expect(() => keptWhole(7, { draft: written, leftOut: [chip] }, true)).toThrow(FilesLeftOut);
  });

  it('hands back the message as written, in the row it was written to', () => {
    // The composer takes it back from here, and must not start a second row.
    let stopped: unknown;
    try {
      keptWhole(7, { draft: written, leftOut: [chip] }, true);
    } catch (e) {
      stopped = e;
    }
    expect(stopped).toBeInstanceOf(FilesLeftOut);
    expect((stopped as FilesLeftOut).draft).toEqual({ ...written, savedId: 7 });
  });

  it('lets an autosave keep what there is', () => {
    expect(keptWhole(7, { draft: written, leftOut: [chip] }, false)).toBe(7);
  });

  it('lets an action go when nothing was left out', () => {
    expect(keptWhole(7, { draft: written, leftOut: [] }, true)).toBe(7);
  });

  it('says nothing of the message in what could reach a log', () => {
    const stopped = new FilesLeftOut(7, {
      ...written,
      attachments: [{ path: '/staged/notes.pdf', name: 'notes.pdf', size: 4 }],
    });
    expect(String(stopped)).toBe('FilesLeftOut: files left out');
  });
});
