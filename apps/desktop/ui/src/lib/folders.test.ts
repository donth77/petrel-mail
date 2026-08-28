import { describe, expect, it } from 'vitest';
import {
  binDestination,
  binTakesFolders,
  buildFolderTree,
  filableFolderRows,
  filableFolders,
  folderDelimiter,
  folderLeaf,
  movedFolderPath,
  movedFolders,
  nameIsTaken,
  nestableRolePath,
  splitPath,
} from './folders';
import type { Folder } from './api';

const f = (id: number, path: string, role = ''): Folder => ({ id, path, role });

/** The user's own Namecheap account, in miniature. */
const namecheap = [
  f(1, 'INBOX', 'inbox'),
  f(2, 'Trash', 'trash'),
  f(3, 'Archive'),
  f(4, 'Archive/Yearly'),
  f(5, 'glassdoor+1032026'),
  f(6, 'Trash/glassdoor+1032026'),
];

describe('the delimiter the server nests with', () => {
  it('is read off real nesting, not guessed from a name', () => {
    expect(folderDelimiter(namecheap)).toBe('/');
  });

  it('is a dot on the servers that nest under INBOX.', () => {
    expect(folderDelimiter([f(1, 'INBOX'), f(2, 'INBOX.Sent'), f(3, 'INBOX.Archive')])).toBe('.');
  });

  it('stays a slash when a folder merely has a dot in its name', () => {
    // `example.com` is an ordinary folder name, and reading it as a
    // hierarchy renamed the folder on the way into the bin.
    const real = [f(1, 'example'), f(2, 'example.com'), f(3, 'Work'), f(4, 'Work/2026')];
    expect(folderDelimiter(real)).toBe('/');
  });

  it('stays a slash when a dot is the only evidence and nothing is nested', () => {
    // Nothing here proves a hierarchy either way, and slash is the answer
    // the rest of the app already assumed — so an unfamiliar server is no
    // worse off, while `example.com` keeps its name.
    expect(folderDelimiter([f(1, 'example'), f(2, 'example.com')])).toBe('/');
  });

  it('answers slash when nothing is nested at all', () => {
    expect(folderDelimiter([f(1, 'INBOX'), f(2, 'Trash')])).toBe('/');
  });
});

describe('a folder’s own name', () => {
  it('is the last segment', () => {
    expect(folderLeaf('Archive/Yearly/2023', '/')).toBe('2023');
    expect(folderLeaf('INBOX.Sent', '.')).toBe('Sent');
    expect(folderLeaf('Receipts', '/')).toBe('Receipts');
  });

  it('keeps the dots that belong to it', () => {
    expect(folderLeaf('Work/example.com', '/')).toBe('example.com');
  });
});

describe('moving a folder to the Trash', () => {
  it('nests it under the trash folder', () => {
    expect(binDestination(namecheap, f(3, 'Archive'))).toBe('Trash/Archive');
  });

  it('numbers it when the bin already holds that name', () => {
    // The refusal this replaces: IMAP answers RENAME onto an occupied name
    // with [ALREADYEXISTS], and the folder never moved.
    expect(binDestination(namecheap, f(5, 'glassdoor+1032026'))).toBe(
      'Trash/glassdoor+1032026 2',
    );
  });

  it('keeps counting when the numbered name is taken too', () => {
    const crowded = [...namecheap, f(7, 'Trash/glassdoor+1032026 2')];
    expect(binDestination(crowded, f(5, 'glassdoor+1032026'))).toBe(
      'Trash/glassdoor+1032026 3',
    );
  });

  it('ignores the folder’s own row, so a re-bin is not renumbered', () => {
    expect(binDestination(namecheap, f(6, 'Trash/glassdoor+1032026'))).toBe(
      'Trash/glassdoor+1032026',
    );
  });

  it('uses the server’s delimiter, not a hard-coded slash', () => {
    const dotted = [f(1, 'INBOX'), f(2, 'INBOX.Trash', 'trash'), f(3, 'INBOX.Receipts')];
    expect(binDestination(dotted, f(3, 'INBOX.Receipts'))).toBe('INBOX.Trash.Receipts');
  });

  it('is nowhere when the account has no trash folder', () => {
    expect(binDestination([f(1, 'INBOX', 'inbox')], f(1, 'INBOX'))).toBeUndefined();
  });

  it('is nowhere on Gmail, where no destination is really the bin', () => {
    // Not because Gmail refuses one — it accepts every destination offered,
    // including a child of [Gmail]/Trash. It just hands back an ordinary
    // label: a message appended there is not in [Gmail]/Trash, not pending
    // purge, and untouched by emptying the Trash. Delete is the only true
    // wording. Proven against the account in live_folder_ops.rs.
    const gmail = [f(1, 'INBOX', 'inbox'), f(2, '[Gmail]/Trash', 'trash'), f(3, 'Notes')];
    expect(binTakesFolders(gmail)).toBe(false);
    expect(binDestination(gmail, f(3, 'Notes'))).toBeUndefined();
  });

  it('is somewhere on a server whose Trash is an ordinary folder', () => {
    expect(binTakesFolders(namecheap)).toBe(true);
  });

  it('leaves Gmail archiving alone, where a label really is filing away', () => {
    const gmail = [
      f(1, 'INBOX', 'inbox'),
      f(2, '[Gmail]/All Mail', 'archive'),
      f(3, 'Notes'),
    ];
    expect(nestableRolePath(gmail, 'archive')).toBe('Archive');
  });
});

describe('moving a folder somewhere else', () => {
  it('nests under the chosen parent', () => {
    expect(movedFolderPath(namecheap, f(5, 'glassdoor+1032026'), 'Archive')).toBe(
      'Archive/glassdoor+1032026',
    );
  });

  it('takes a folder back to the top level', () => {
    expect(movedFolderPath(namecheap, f(4, 'Archive/Yearly'), '')).toBe('Yearly');
  });
});

describe('reading the server’s refusal', () => {
  it('recognises an occupied name', () => {
    expect(
      nameIsTaken(
        'imap: no response: code: None, info: Some("[ALREADYEXISTS] Target mailbox already exists")',
      ),
    ).toBe(true);
  });

  it('does not claim every failure is a name clash', () => {
    expect(nameIsTaken('imap: io: Broken pipe (os error 32)')).toBe(false);
  });
});

describe('the tree a move produces, drawn before the server is asked', () => {
  it('takes the subtree with it, the way a RENAME does', () => {
    // Archive (3) and its child Archive/Yearly (4) move under INBOX.
    const next = movedFolders(namecheap, 3, 'INBOX/Archive');
    expect(next.find((x) => x.id === 3)?.path).toBe('INBOX/Archive');
    expect(next.find((x) => x.id === 4)?.path).toBe('INBOX/Archive/Yearly');
  });

  it('leaves every other folder exactly as it was', () => {
    const next = movedFolders(namecheap, 3, 'INBOX/Archive');
    for (const id of [1, 2, 5, 6]) {
      expect(next.find((x) => x.id === id)).toEqual(namecheap.find((x) => x.id === id));
    }
  });

  it('does not mistake a name that merely starts the same for a child', () => {
    const folders = [f(1, 'Work'), f(2, 'Workshop'), f(3, 'Work/Notes')];
    const next = movedFolders(folders, 1, 'Archive/Work');
    expect(next.find((x) => x.id === 2)?.path).toBe('Workshop');
    expect(next.find((x) => x.id === 3)?.path).toBe('Archive/Work/Notes');
  });

  it('hands back what it was given when nothing moves', () => {
    expect(movedFolders(namecheap, 3, 'Archive')).toBe(namecheap);
    expect(movedFolders(namecheap, 999, 'Archive')).toBe(namecheap);
  });
});

describe('the folders mail can be filed into', () => {
  it('drops the role mailboxes, which all have verbs of their own', () => {
    const paths = filableFolders(namecheap).map((x) => x.path);
    expect(paths).not.toContain('INBOX');
    expect(paths).not.toContain('Trash');
  });

  it('keeps what is in the bin, which the tree folds away rather than hides', () => {
    // This used to be excluded, because fifty dead alias folders made a flat
    // list unreadable. The tree answers that better: Trash arrives folded.
    expect(filableFolders(namecheap).map((x) => x.path)).toContain('Trash/glassdoor+1032026');
    // The bin itself is still not a destination — it is the rung they hang on.
    expect(filableFolders(namecheap).map((x) => x.path)).not.toContain('Trash');
  });

  it('keeps the ordinary folders, nesting and all', () => {
    // Archive itself is missing on purpose. Namecheap marks no \Archive, so a
    // plain top-level Archive is the anchor by convention — the rail already
    // draws it as the Archive mailbox, and offering it here would be the same
    // place twice. What hangs under it is a place of its own.
    expect(filableFolders(namecheap).map((x) => x.path)).toEqual([
      'Archive/Yearly',
      'glassdoor+1032026',
      'Trash/glassdoor+1032026',
    ]);
  });

  it('drops the Gmail anchor labels, because Archive is already a verb', () => {
    const gmail = [
      f(1, 'INBOX', 'inbox'),
      f(2, '[Gmail]/All Mail', 'archive'),
      f(3, 'Archive'),
      f(4, 'Archive/2026'),
      f(5, 'Receipts'),
    ];
    // `Archive` is the anchor the first nested label created, not a place of
    // its own; `Archive/2026` under it is.
    expect(filableFolders(gmail).map((x) => x.path)).toEqual(['Archive/2026', 'Receipts']);
  });
});

describe('a path split into context and name', () => {
  it('keeps the delimiter with the parent, so the halves still join up', () => {
    const { parent, leaf } = splitPath('Archive/Yearly/2023/Job Hunt 2023');
    expect(parent).toBe('Archive/Yearly/2023/');
    expect(leaf).toBe('Job Hunt 2023');
    // Load-bearing: the pickers map fuzzy-match indices onto the two spans by
    // the length of the first, which only works if nothing is dropped.
    expect(parent + leaf).toBe('Archive/Yearly/2023/Job Hunt 2023');
  });

  it('reads either separator, because which one it is belongs to the server', () => {
    expect(splitPath('INBOX.Receipts')).toEqual({ parent: 'INBOX.', leaf: 'Receipts' });
  });

  it('calls a name with no separator all leaf — which is also right for a tag', () => {
    expect(splitPath('Receipts')).toEqual({ parent: '', leaf: 'Receipts' });
    expect(splitPath('Waiting on')).toEqual({ parent: '', leaf: 'Waiting on' });
  });

  it('splits at the last separator, not the first', () => {
    expect(splitPath('Archive/Outdated/Hacker Paradise')).toEqual({
      parent: 'Archive/Outdated/',
      leaf: 'Hacker Paradise',
    });
  });
});

describe('the hierarchy the paths already spell', () => {
  it('nests a path under the rungs it names', () => {
    const tree = buildFolderTree([f(1, 'Archive'), f(2, 'Archive/Yearly'), f(3, 'Archive/Yearly/2023')]);
    expect(tree).toHaveLength(1);
    expect(tree[0].label).toBe('Archive');
    expect(tree[0].children[0].label).toBe('Yearly');
    expect(tree[0].children[0].children[0].label).toBe('2023');
    expect(tree[0].children[0].children[0].folder?.id).toBe(3);
  });

  it('keeps the order it was given, because that order is what a drag saved', () => {
    // Alphabetising here is what once threw away a reordering the engine had
    // faithfully stored.
    const tree = buildFolderTree([f(1, 'Zebra'), f(2, 'Apple'), f(3, 'Mango')]);
    expect(tree.map((n) => n.label)).toEqual(['Zebra', 'Apple', 'Mango']);
  });

  it('invents a rung for a parent that never arrived, and keeps it only if used', () => {
    const tree = buildFolderTree([f(1, 'Archive/Yearly/2023')]);
    expect(tree[0].label).toBe('Archive');
    expect(tree[0].folder).toBeUndefined();
    expect(tree[0].children[0].children[0].folder?.id).toBe(1);
  });

  it('starts below an anchor when the paths already account for it', () => {
    // How the rail draws archived folders beneath the Archive mailbox row.
    const tree = buildFolderTree([f(4, 'Archive/Yearly'), f(5, 'Archive/Yearly/2023')], 'Archive'.length);
    expect(tree.map((n) => n.label)).toEqual(['Yearly']);
    expect(tree[0].children[0].label).toBe('2023');
  });
});

describe('the filable folders, flattened back out with their depth', () => {
  it('walks depth-first and marks the rungs that are not destinations', () => {
    // Archive is the anchor, so it is not filable — but Archive/Old letters is,
    // and it must not be indented under nothing.
    const folders = [
      f(1, 'INBOX', 'inbox'),
      f(2, 'Archive'),
      f(3, 'Archive/Old letters'),
      f(4, 'Archive/Old letters/2019'),
      f(5, 'Receipts'),
    ];
    // Receipts first: Archive is an anchor and anchors sink, so what you file
    // into day to day comes before what you have already dealt with.
    expect(filableFolderRows(folders).map((r) => [r.path, r.depth, r.container])).toEqual([
      ['Receipts', 0, false],
      ['Archive', 0, true],
      ['Archive/Old letters', 1, false],
      ['Archive/Old letters/2019', 2, false],
    ]);
  });

  it('gives every container a distinct id that is never a folder id', () => {
    const rows = filableFolderRows([f(1, 'Archive'), f(2, 'Archive/A/x'), f(3, 'Archive/B/y')]);
    const containers = rows.filter((r) => r.container).map((r) => r.id);
    expect(containers.every((id) => id < 0)).toBe(true);
    expect(new Set(containers).size).toBe(containers.length);
  });
});

describe('where Archive and Trash sit in a picker', () => {
  const account = [
    f(1, 'INBOX', 'inbox'),
    f(2, 'Trash', 'trash'),
    f(3, 'Archive'),
    f(4, 'Archive/Yearly'),
    f(5, 'Receipts'),
    f(6, 'Trash/dead alias'),
  ];

  it('sinks them to the bottom, with everything under them', () => {
    // 32 of 38 folders on a real mailbox live under Archive. Left in
    // alphabetical place it buries the handful you actually file into.
    expect(filableFolderRows(account).map((r) => r.path)).toEqual([
      'Receipts',
      'Archive',
      'Archive/Yearly',
      'Trash',
      'Trash/dead alias',
    ]);
  });

  it('marks them, so they can wear their own glyph and arrive folded', () => {
    const rows = filableFolderRows(account);
    expect(rows.find((r) => r.path === 'Archive')?.anchor).toBe('archive');
    expect(rows.find((r) => r.path === 'Trash')?.anchor).toBe('trash');
    expect(rows.find((r) => r.path === 'Receipts')?.anchor).toBeUndefined();
  });

  it('says which rows have something to fold', () => {
    const rows = filableFolderRows(account);
    expect(rows.find((r) => r.path === 'Archive')?.hasChildren).toBe(true);
    expect(rows.find((r) => r.path === 'Receipts')?.hasChildren).toBe(false);
  });
});
