import { describe, expect, it } from 'vitest';
import { exportScopes } from './export-scopes';
import type { Folder } from './api';

const f = (id: number, path: string, role = ''): Folder => ({ id, path, role });

/** A mailbox that files its history under Archive, as a real one does. */
const folders = [
  f(1, 'INBOX', 'inbox'),
  f(2, 'Archive', 'archive'),
  f(3, 'Archive/2023'),
  f(4, 'Archive/2024'),
  f(5, 'Receipts'),
  f(6, 'Trash', 'trash'),
  f(7, 'Trash/Old aliases'),
];

const MAILBOXES = [
  { view: 'inbox', label: 'Inbox' },
  { view: 'archive', label: 'Archive' },
  { view: 'trash', label: 'Trash' },
];

const scopes = () => exportScopes(MAILBOXES, folders, [{ id: 9, name: 'urgent', colour: '#b00' }]);

describe('exportScopes', () => {
  it('draws Archive and Trash once, not once per role they play', () => {
    for (const name of ['Archive', 'Trash']) {
      expect(scopes().filter((s) => s.label === name)).toHaveLength(1);
    }
  });

  it('leaves the one row choosable, and meaning the whole mailbox', () => {
    const archive = scopes().find((s) => s.label === 'Archive')!;
    expect(archive.view).toBe('archive');
    expect(archive.container).toBeUndefined();
    // And still the thing its subfolders hang under.
    expect(archive.hasChildren).toBe(true);
    expect(archive.anchor).toBe('archive');
  });

  it('puts the subfolders under the mailbox row, in the mailbox’s place', () => {
    const rows = scopes();
    const at = rows.findIndex((s) => s.label === 'Archive');
    expect(rows[at + 1].label).toBe('Archive/2023');
    expect(rows[at + 2].label).toBe('Archive/2024');
    expect(rows[at + 1].view).toBe('folder:3');
    expect(rows[at + 1].depth).toBe(1);
    // Trash follows, because the mailbox order decides where they sit — not
    // the folder tree's own sort, which puts both anchors at the bottom.
    expect(rows[at + 3].label).toBe('Trash');
  });

  it('keeps a mailbox in its place when nothing is filed under it', () => {
    const bare = [f(1, 'INBOX', 'inbox'), f(2, 'Archive', 'archive'), f(3, 'Receipts')];
    const rows = exportScopes(MAILBOXES, bare, []);
    const archive = rows.find((s) => s.view === 'archive')!;
    expect(archive.label).toBe('Archive');
    expect(archive.hasChildren).toBeUndefined();
    expect(rows.findIndex((s) => s.view === 'archive')).toBe(1);
  });

  it('lists the ordinary folders after the mailboxes, and the tags last', () => {
    const rows = scopes();
    const receipts = rows.findIndex((s) => s.label === 'Receipts');
    const archive = rows.findIndex((s) => s.label === 'Archive');
    const tag = rows.findIndex((s) => s.view === 'tag:urgent');
    expect(archive).toBeLessThan(receipts);
    expect(receipts).toBeLessThan(tag);
    expect(rows[tag].colour).toBe('#b00');
  });

  it('does not list a folder twice by leaving it in both sections', () => {
    const labels = scopes().map((s) => s.label);
    expect(new Set(labels).size).toBe(labels.length);
  });
});
