/* The IPC seam. Every call into the engine goes through here, so the surface the
   UI depends on is one file wide (ADR-0005). Types mirror the Rust structs. */

import { invoke } from '@tauri-apps/api/core';

export type Listing = {
  id: number;
  from_display: string;
  from_addr: string;
  subject: string;
  snippet: string;
  date_ms: number;
};

/** A conversation row. The list shows these, not individual messages. */
export type Thread = {
  thread_id: number;
  /** Newest message in the conversation — what the row displays and opens. */
  id: number;
  from_display: string;
  from_addr: string;
  subject: string;
  snippet: string;
  date_ms: number;
  message_count: number;
  participants: string;
  unread: boolean;
  starred: boolean;
  has_attachments: boolean;
  tags: { name: string; colour: string }[];
  attachment_name: string | null;
};

export type Tag = { id: number; name: string; colour: string; thread_count: number };

export type Attachment = { filename: string; size: number };

export type ThreadMessage = {
  id: number;
  from_display: string;
  from_addr: string;
  subject: string;
  snippet: string;
  date_ms: number;
  unread: boolean;
  recipients: string[];
  attachments: Attachment[];
};

export type FolderMapping = { role: string; path: string };

export type Account = {
  id: number;
  kind: string;
  email: string;
  display_name: string;
  color: string;
  local_archive: boolean;
  message_count: number;
  unread_count: number;
  newest_ms: number | null;
  folders: FolderMapping[];
};

export type Identity = {
  address: string;
  display_name: string;
  signature: string;
  signature_on_reply: boolean;
};

export type StorageReport = {
  messages: number;
  attachments: number;
  database_bytes: number;
  blob_bytes: number;
  index_bytes: number;
};

export type Folder = { id: number; role: string; path: string };

export type ActionKind =
  | 'archive' | 'trash' | 'spam' | 'star' | 'unstar' | 'mark_read' | 'mark_unread'
  // These three carry a target id alongside — a folder for move, a tag for the
  // other two. The kind stays a plain string so every action has one wire shape.
  | 'move' | 'tag' | 'untag'
  // Local only: the target is the instant to come back at.
  | 'snooze' | 'unsnooze';

export type ActionReceipt = {
  action_id: number;
  kind: ActionKind;
  message_count: number;
  /** Already past tense: by the time this arrives, it has happened. */
  description: string;
};

export type Status = {
  /** Present when a sync failed. A login that fails must not read as an empty
   *  mailbox — the two look identical until something says so. */
  sync_error?: string | null;
  seeding: boolean;
  count: number;
  source: string;
  retention: string;
  data_dir: string;
};

/* Dev-only: `npm run dev` opens in a plain browser, where Tauri's invoke does
   not exist. Rather than staring at a permanently failing page while iterating
   on layout, serve synthetic rows. `import.meta.env.DEV` is compiled out of
   production builds, so none of this ships. */
const inTauri = () => typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

const NAMES = [
  ['Sam Ortiz', 'sam@vendorco.example'], ['Dana Wu', 'dana@northbay.example'],
  ['The Weekly Ledger', 'news@ledger.example'], ['Depot Supply', 'orders@depot.example'],
  ['Priya Raman', 'priya@clientco.example'], ['会議事務局', 'kaigi@example.jp'],
];
const SUBJECTS = [
  'Q3 vendor contracts — pricing before Friday', 'Re: Vendor shortlist',
  'Issue 214 — what the rate cut means', 'Your order has shipped',
  'Notes from Tuesday', '東京支社の会議について',
];
function mockRows(n: number, offset = 0): Thread[] {
  return Array.from({ length: n }, (_, i) => {
    const k = offset + i;
    const [display, addr] = NAMES[k % NAMES.length];
    return {
      thread_id: k + 1,
      id: k + 1,
      from_display: display,
      from_addr: addr,
      subject: `${SUBJECTS[k % SUBJECTS.length]}${k > 5 ? ` (${k})` : ''}`,
      snippet: 'the twelve-month term works, and the volume tier resets annually rather than…',
      date_ms: Date.now() - k * 37 * 60 * 1000,
      message_count: [1, 1, 4, 1, 2, 1, 7][k % 7],
      participants: k % 3 === 2 ? `${display}, Dana Wu, you` : display,
      unread: k % 3 === 2,
      starred: k % 9 === 4,
      has_attachments: k % 5 === 0,
      tags:
        k % 4 === 0
          ? [{ name: 'urgent', colour: '#B0524A' }]
          : k % 7 === 3
            ? [{ name: 'receipts', colour: '#5E7C4A' }, { name: 'read later', colour: '#9A6B1F' }]
            : [],
      attachment_name: k % 5 === 0 ? 'contract-v3.pdf' : null,
    };
  });
}

const mockAccounts: Account[] = [
  {
    id: 1, kind: 'imap', email: 'tom@northbay.example', display_name: 'Work',
    color: '#0E7C86', local_archive: false, message_count: 8421, unread_count: 9,
    newest_ms: Date.now() - 4 * 60000,
    folders: [
      { role: 'archive', path: '[Gmail]/All Mail' },
      { role: 'drafts', path: '[Gmail]/Drafts' },
      { role: 'sent', path: '[Gmail]/Sent Mail' },
      { role: 'spam', path: '[Gmail]/Spam' },
      { role: 'trash', path: '[Gmail]/Trash' },
    ],
  },
];

const mock = {
  status: async (): Promise<Status> => ({
    seeding: false, count: 10000, source: 'tom@northbay.example',
    retention: 'mirror', data_dir: '~/Library/Application Support/Petrel',
  }),
  threads: async (view: string, offset: number, limit: number) => {
    const rows = mockRows(Math.min(limit, 2000), offset);
    // Enough fidelity to exercise the view switch: the browser mock is not the
    // engine, and pretending otherwise is what let an unimplemented view look
    // implemented.
    if (view === 'starred') return rows.filter((r) => r.starred);
    if (view === 'snoozed' || view === 'outbox') return [];
    if (view.startsWith('tag:')) {
      const name = view.slice(4);
      return rows.filter((r) => r.tags.some((t) => t.name === name));
    }
    if (view !== 'inbox') return [];
    return rows;
  },
  search: async (q: string) =>
    mockRows(24).filter((r) => r.subject.toLowerCase().includes(q.toLowerCase())),
  // Fresh objects each call: returning the same reference means React sees no
  // change and the mock silently misreports whether a write took effect.
  triage: async (_t: number, kind: ActionKind, _target?: number): Promise<ActionReceipt> => ({
    action_id: Math.floor(Math.random() * 1e6),
    kind,
    message_count: 1,
    description:
      { archive: 'Archived', trash: 'Moved to Trash', spam: 'Reported as spam',
        star: 'Starred', unstar: 'Unstarred', mark_read: 'Marked read',
        mark_unread: 'Marked unread', move: 'Moved', tag: 'Tagged',
        untag: 'Untagged', snooze: 'Snoozed', unsnooze: 'Back in the inbox' }[kind],
  }),
  undoTriage: async () => true,
  folders: async (): Promise<Folder[]> => [
    { id: 101, role: '', path: 'Contracts' },
    { id: 102, role: '', path: 'Contracts/2026' },
    { id: 103, role: '', path: 'Client contact' },
    { id: 1, role: 'archive', path: 'Archive' },
  ],
  createFolder: async () => 999,
  createTag: async () => 998,
  send: async () => 'mock-message-id@example.com',
  storage: async (): Promise<StorageReport> => ({
    messages: 40, attachments: 2,
    database_bytes: 12_582_912, blob_bytes: 41_943_040, index_bytes: 3_145_728,
  }),
  exportMbox: async () => '40/0',
  identity: async (): Promise<Identity> => ({
    address: 'you@example.com', display_name: 'You', signature: '', signature_on_reply: false,
  }),
  setIdentity: async () => {},
  attachmentInfo: async (paths: string[]) =>
    paths.map((path) => ({ path, name: path.split('/').pop() || path, size: 1024 })),
  accounts: async (): Promise<Account[]> => mockAccounts.map((a) => ({ ...a })),
  setAccountColor: async (id: number, color: string) => {
    const a = mockAccounts.find((x) => x.id === id);
    if (a) a.color = color;
  },
  setAccountArchive: async (id: number, enabled: boolean) => {
    const a = mockAccounts.find((x) => x.id === id);
    if (a) a.local_archive = enabled;
  },
  getSettings: async (): Promise<Record<string, string>> => ({}),
  setSetting: async () => {},
  tags: async (): Promise<Tag[]> => [
    { id: 1, name: 'read later', colour: '#9A6B1F', thread_count: 12 },
    { id: 2, name: 'receipts', colour: '#5E7C4A', thread_count: 31 },
    { id: 3, name: 'urgent', colour: '#B0524A', thread_count: 4 },
  ],
  threadDetail: async (): Promise<ThreadMessage[]> => [
    {
      id: 1, from_display: 'Dana Wu', from_addr: 'dana@northbay.example',
      subject: 'Q3 vendor contracts', snippet: 'Sending the draft ahead of Friday so you both have time…',
      date_ms: Date.now() - 3 * 86400000, unread: false,
      recipients: ['Sam Ortiz', 'me'], attachments: [],
    },
    {
      id: 2, from_display: 'Sam Ortiz', from_addr: 'sam@vendorco.example',
      subject: 'Re: Q3 vendor contracts', snippet: 'I have marked up section 4…',
      date_ms: Date.now() - 90 * 60000, unread: true,
      recipients: ['Dana Wu', 'me'],
      attachments: [
        { filename: 'contract-v3.pdf', size: 2202009 },
        { filename: 'annex-logistics.xlsx', size: 49152 },
      ],
    },
  ],
  // A real document carrying the same injected script the petrel-msg: handler
  // adds, so the browser reproduces the app's structure — including a focused
  // frame that would otherwise swallow every shortcut. Returning '' meant the
  // browser had no iframe at all, which hid that entire class of bug.
  messageUrl: async () =>
    'data:text/html,' +
    encodeURIComponent(
      '<!doctype html><meta charset=utf-8>' +
        '<body style="margin:0;padding:14px 16px;font:14px/1.6 system-ui;color:#182730">' +
        '<p>Sam — the twelve-month term works. I will get the annex signed off today.</p>' +
        '<p>On the volume tier: it resets annually, not quarterly.</p>' +
        '<script>' +
        'function h(){var d=document.documentElement;return d.scrollHeight}' +
        "addEventListener('load',function(){parent.postMessage({petrelHeight:h()},'*')});" +
        "addEventListener('keydown',function(e){parent.postMessage({petrelKey:{" +
        'key:e.key,metaKey:e.metaKey,ctrlKey:e.ctrlKey,shiftKey:e.shiftKey,altKey:e.altKey' +
        "}},'*')});" +
        '<\/script></body>',
    ),
  log: async () => {},
};

const real = {
  status: () => invoke<Status>('status'),
  threads: (view: string, offset: number, limit: number) =>
    invoke<Thread[]>('list_threads', { view, offset, limit }),
  tags: () => invoke<Tag[]>('list_tags'),
  triage: (threadId: number, kind: ActionKind, target?: number) =>
    invoke<ActionReceipt>('triage', { threadId, kind, target: target ?? null }),
  undoTriage: (actionId: number) => invoke<boolean>('undo_triage', { actionId }),
  folders: () => invoke<Folder[]>('list_folders'),
  createFolder: (path: string) => invoke<number>('create_folder', { path }),
  createTag: (name: string) => invoke<number>('create_tag', { name }),
  send: (
    to: string[], cc: string[], subject: string, body: string,
    inReplyTo: string | null, references: string[], attachments: string[],
  ) =>
    invoke<string>('send_message', {
      to, cc, subject, body, inReplyTo, references, attachments,
    }),
  storage: () => invoke<StorageReport>('storage_report'),
  exportMbox: (view: string, path: string) => invoke<string>('export_mbox', { view, path }),
  identity: () => invoke<Identity>('get_identity'),
  setIdentity: (displayName: string, signature: string, signatureOnReply: boolean) =>
    invoke<void>('set_identity', { displayName, signature, signatureOnReply }),
  attachmentInfo: (paths: string[]) =>
    invoke<{ path: string; name: string; size: number }[]>('attachment_info', { paths }),
  accounts: () => invoke<Account[]>('list_accounts'),
  setAccountColor: (accountId: number, color: string) =>
    invoke<void>('set_account_color', { accountId, color }),
  setAccountArchive: (accountId: number, enabled: boolean) =>
    invoke<void>('set_account_archive', { accountId, enabled }),
  getSettings: () => invoke<Record<string, string>>('get_settings'),
  setSetting: (key: string, value: string) => invoke<void>('set_setting', { key, value }),
  threadDetail: (threadId: number) =>
    invoke<ThreadMessage[]>('thread_detail', { threadId }),
  search: (query: string) => invoke<Thread[]>('search_messages', { query }),

  messageUrl: (messageId: number) => invoke<string>('message_url', { messageId }),
  log: (entry: string) => invoke('frontend_log', { entry }).catch(() => {}),
};

export const api = import.meta.env.DEV && !inTauri() ? mock : real;
