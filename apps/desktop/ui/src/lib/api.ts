/* The IPC seam. Every call into the engine goes through here, so the surface the
   UI depends on is one file wide. Types mirror the Rust structs. */

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
  /** The tag's id travels with the row so it can be untagged directly, rather
      than by finding its name in the rail's list. */
  tags: { id: number; name: string; colour: string }[];
  attachment_name: string | null;
  /** Why this row matched, when it came from a search: the text around the hit
   *  with the matched words wrapped in `[` and `]`. Null in an ordinary list. */
  match_snippet: string | null;
};

export type Tag = { id: number; name: string; colour: string; thread_count: number };

export type Attachment = { filename: string; size: number };

/** A message read back for quoting: sanitized, with remote content stripped. */
/** One server, as discovery found it or the form typed it. */
export type Server = { host: string; port: number; tls: boolean };

/** What discovery found for an address. */
export type Discovered = {
  provider: string;
  via: 'known-provider' | 'ispdb' | 'mx';
  imap: Server;
  smtp: Server;
  auth: 'password' | 'app-password';
  app_password_url: string | null;
};

/** Everything needed to test and then store an account. */
export type AccountSetup = {
  email: string;
  username: string;
  password: string;
  imap_host: string;
  imap_port: number;
  smtp_host: string;
  smtp_port: number;
  provider: string;
};

/** One message in the outbox, with where its send attempt left it. */
export type OutboxRow = {
  id: number;
  subject: string;
  to: string;
  send_after_ms: number;
  /** `UndoWindow` | `Transmitting` | `RetryQueued` | `FailedPermanent` | `NeedsAttention` */
  state: string;
  error: string | null;
  attempts: number;
  next_ms: number | null;
  attachments: number;
};

export type Quoted = {
  html: string;
  text: string;
  from: string;
  date_ms: number;
  to: string;
  subject: string;
};

/** Somebody worth offering while a recipient is typed. */
export type Correspondent = { addr: string; display: string; written_to: boolean };

/** Whether a message's remote content may load, and on what grounds. */
export type RemoteStatus = {
  from_addr: string;
  allowed: boolean;
  /** Allowed because the user has written to them, not by an explicit choice —
   *  so there is nothing in the trusted list to find or undo. */
  because_written_to: boolean;
};

export type ThreadMessage = {
  id: number;
  from_display: string;
  from_addr: string;
  subject: string;
  snippet: string;
  date_ms: number;
  unread: boolean;
  recipients: string[];
  recipient_addrs: string[];
  attachments: Attachment[];
};

export type FolderMapping = { role: string; path: string };

export type Account = {
  id: number;
  /** The one the window shows. */
  active: boolean;
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

export type DraftRecord = {
  id: number;
  to: string;
  subject: string;
  /** Plain text: the snippet, the search index, and the text half that is sent. */
  body: string;
  /** The rich half; empty for a draft written before there was one. */
  html: string;
};;

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
  | 'snooze' | 'unsnooze'
  // The one with no inverse. Confirmed before, never offered as undo after.
  | 'delete_forever';

export type ActionReceipt = {
  action_id: number;
  kind: ActionKind;
  message_count: number;
  /** Already past tense: by the time this arrives, it has happened. */
  description: string;
};

export type Status = {
  /** Whether any account can sign in. `false` means first run: show
   *  onboarding rather than an empty mailbox. */
  configured: boolean;
  /** Present when a sync failed. A login that fails must not read as an empty
   *  mailbox — the two look identical until something says so. */
  sync_error?: string | null;
  seeding: boolean;
  count: number;
  /** What the server holds across the synced folders, or 0 before it is asked.
   *  The denominator of the coverage line under search results. */
  server_total: number;
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
          ? [{ id: 301, name: 'urgent', colour: '#B0524A' }]
          : k % 7 === 3
            ? [
                { id: 303, name: 'receipts', colour: '#5E7C4A' },
                { id: 304, name: 'read later', colour: '#9A6B1F' },
              ]
            : [],
      attachment_name: k % 5 === 0 ? 'contract-v3.pdf' : null,
      // An ordinary list, so nothing matched anything.
      match_snippet: null,
    };
  });
}

const mockAccounts: Account[] = [
  {
    id: 1, kind: 'imap', email: 'tom@northbay.example', display_name: 'Work',
    color: '#0E7C86', local_archive: false, message_count: 8421, unread_count: 9, active: true,
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
    configured: true,
    seeding: false, count: 10000, server_total: 12500, source: 'tom@northbay.example',
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
    mockRows(24)
      .filter((r) => r.subject.toLowerCase().includes(q.toLowerCase()))
      .map((r) => ({
        ...r,
        match_snippet: `…the \u{E000}${q}\u{E001} you were looking for…`,
      })),
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
        untag: 'Untagged', snooze: 'Snoozed', unsnooze: 'Back in the inbox',
        delete_forever: 'Deleted' }[kind],
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
  renameTag: async () => {},
  setTagColour: async () => {},
  deleteTag: async () => {},
  storage: async (): Promise<StorageReport> => ({
    messages: 40, attachments: 2,
    database_bytes: 12_582_912, blob_bytes: 41_943_040, index_bytes: 3_145_728,
  }),
  exportMbox: async () => '40/0',
  identity: async (): Promise<Identity> => ({
    address: 'you@example.com', display_name: 'You', signature: '', signature_on_reply: false,
  }),
  setIdentity: async () => {},
  saveDraft: async () => 1,
  loadDraft: async (): Promise<DraftRecord> => ({ id: 1, to: '', subject: '', body: '', html: '' }),
  deleteDraft: async () => {},
  scheduleSend: async () => {},
  popoutCompose: async () => {},
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
  viewCounts: async (mode: string): Promise<[string, number][]> =>
    mode === 'off' ? [] : [['inbox', 3], ['drafts', 1], ['spam', 2]],
  popoutMessage: async () => {},
  quoteMessage: async (): Promise<Quoted> => ({
    to: 'Sam Ortiz <sam@example.com>',
    subject: 'Q3 vendor contracts',
    html: '<p>The original message, as it was written.</p>',
    text: 'The original message, as it was written.',
    from: 'Dana Wu',
    date_ms: Date.now() - 3600_000,
  }),
  completeAddresses: async (prefix: string): Promise<Correspondent[]> =>
    [
      { addr: 'nadia@example.com', display: 'Nadia Okafor', written_to: true },
      { addr: 'news@example.com', display: 'News Digest', written_to: false },
    ].filter((c) => c.addr.startsWith(prefix) || c.display.toLowerCase().includes(prefix)),
  remoteStatus: async (): Promise<RemoteStatus> => ({
    from_addr: 'sam@example.com', allowed: false, because_written_to: false,
  }),
  showRemoteOnce: async () => {},
  trustSender: async () => 'sam@example.com',
  trustedSenders: async (): Promise<string[]> => [],
  untrustSender: async () => {},
  // Searched across every row the mock can produce, not through one view —
  // finding a conversation wherever it lives is the whole point of the call.
  stageAttachment: async (name: string, bytes: Uint8Array) => ({
    path: `/tmp/staged/${name}`, name, size: bytes.length,
  }),
  discoverAccount: async (address: string): Promise<Discovered | null> =>
    address.endsWith('@gmail.com')
      ? { provider: 'Gmail', via: 'known-provider', imap: { host: 'imap.gmail.com', port: 993, tls: true },
          smtp: { host: 'smtp.gmail.com', port: 465, tls: true }, auth: 'app-password',
          app_password_url: 'https://myaccount.google.com/apppasswords' }
      : null,
  guessServers: async (address: string): Promise<[Server, Server] | null> => {
    const d = address.split('@')[1];
    return d ? [{ host: `imap.${d}`, port: 993, tls: true }, { host: `smtp.${d}`, port: 465, tls: true }] : null;
  },
  testAccount: async () => {},
  addAccount: async () => 2,
  removeAccount: async () => {},
  setActiveAccount: async () => {},
  outbox: async (): Promise<OutboxRow[]> => {
    const now = Date.now();
    return [
      { id: 901, subject: 'Re: Q3 vendor contracts — pricing before Friday', to: 'Sam Ortiz, Dana Wu',
        send_after_ms: now + 7000, state: 'UndoWindow', error: null, attempts: 0, next_ms: null, attachments: 0 },
      { id: 902, subject: 'Invoice 2214', to: 'accounts@clientco.example',
        send_after_ms: now - 60000, state: 'RetryQueued', error: 'connect: network unreachable',
        attempts: 1, next_ms: now + 30000, attachments: 1 },
      { id: 903, subject: 'Notes from Tuesday', to: 'maya@northbay.example',
        send_after_ms: now - 120000, state: 'RetryQueued', error: 'connect: connection refused',
        attempts: 2, next_ms: now + 120000, attachments: 0 },
      { id: 904, subject: 'Board pack v4', to: 'directors@northbay.example',
        send_after_ms: now - 300000, state: 'NeedsAttention', error: 'connection closed after DATA',
        attempts: 1, next_ms: null, attachments: 2 },
      { id: 905, subject: 'Welcome aboard!', to: 'j.smith@oldcompany.example',
        send_after_ms: now - 400000, state: 'FailedPermanent', error: '550 — no such user here',
        attempts: 1, next_ms: null, attachments: 0 },
    ];
  },
  outboxSendNow: async () => {},
  outboxEdit: async () => {},
  outboxCheck: async () => 'NeedsAttention',
  openExternal: async (url: string) => {
    // The browser stand-in cannot hand a URL to the system, and opening one in
    // the harness tab would navigate away from the app under test.
    console.info('[mock] would open externally:', url);
  },
  threadById: async (threadId: number): Promise<Thread | null> =>
    mockRows(500).find((r: Thread) => r.thread_id === threadId) ?? null,
  threadDetail: async (): Promise<ThreadMessage[]> => [
    {
      id: 1, from_display: 'Dana Wu', from_addr: 'dana@northbay.example',
      subject: 'Q3 vendor contracts', snippet: 'Sending the draft ahead of Friday so you both have time…',
      date_ms: Date.now() - 3 * 86400000, unread: false,
      recipients: ['Sam Ortiz', 'me'], recipient_addrs: ['sam@example.com', 'you@example.com'], attachments: [],
    },
    {
      id: 2, from_display: 'Sam Ortiz', from_addr: 'sam@vendorco.example',
      subject: 'Re: Q3 vendor contracts', snippet: 'I have marked up section 4…',
      date_ms: Date.now() - 90 * 60000, unread: true,
      recipients: ['Dana Wu', 'me'],
      recipient_addrs: ['dana@example.com', 'you@example.com'],
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
        // The backslash is what stops this literal ending the <script> block it
        // is written inside.
        // eslint-disable-next-line no-useless-escape
        '<\/script></body>',
    ),
  log: async () => {},
};

const real = {
  status: () => invoke<Status>('status'),
  threads: (view: string, offset: number, limit: number) =>
    invoke<Thread[]>('list_threads', { view, offset, limit }),
  threadById: (threadId: number) => invoke<Thread | null>('thread_by_id', { threadId }),
  openExternal: (url: string) => invoke<void>('open_external', { url }),
  discoverAccount: (address: string) => invoke<Discovered | null>('discover_account', { address }),
  guessServers: (address: string) => invoke<[Server, Server] | null>('guess_servers', { address }),
  testAccount: (setup: AccountSetup, which?: 'imap' | 'smtp') =>
    invoke<void>('test_account', { setup, which: which ?? null }),
  addAccount: (setup: AccountSetup) => invoke<number>('add_account', { setup }),
  removeAccount: (accountId: number) => invoke<void>('remove_account', { accountId }),
  setActiveAccount: (accountId: number) => invoke<void>('set_active_account', { accountId }),
  outbox: () => invoke<OutboxRow[]>('list_outbox'),
  outboxSendNow: (id: number) => invoke<void>('outbox_send_now', { id }),
  outboxEdit: (id: number) => invoke<void>('outbox_edit', { id }),
  outboxCheck: (id: number) => invoke<string>('outbox_check', { id }),
  stageAttachment: (name: string, bytes: Uint8Array) =>
    invoke<{ path: string; name: string; size: number }>('stage_attachment', { name, bytes }),
  tags: () => invoke<Tag[]>('list_tags'),
  viewCounts: (mode: string) => invoke<[string, number][]>('view_counts', { mode }),
  popoutMessage: (threadId: number) => invoke<void>('popout_message', { threadId }),
  quoteMessage: (messageId: number) => invoke<Quoted>('quote_message', { messageId }),
  completeAddresses: (prefix: string) =>
    invoke<Correspondent[]>('complete_addresses', { prefix }),
  remoteStatus: (messageId: number) => invoke<RemoteStatus>('remote_status', { messageId }),
  showRemoteOnce: (messageId: number) => invoke<void>('show_remote_once', { messageId }),
  trustSender: (messageId: number) => invoke<string>('trust_sender', { messageId }),
  trustedSenders: () => invoke<string[]>('trusted_senders'),
  untrustSender: (addr: string) => invoke<void>('untrust_sender', { addr }),
  triage: (threadId: number, kind: ActionKind, target?: number) =>
    invoke<ActionReceipt>('triage', { threadId, kind, target: target ?? null }),
  undoTriage: (actionId: number) => invoke<boolean>('undo_triage', { actionId }),
  folders: () => invoke<Folder[]>('list_folders'),
  createFolder: (path: string) => invoke<number>('create_folder', { path }),
  createTag: (name: string) => invoke<number>('create_tag', { name }),
  renameTag: (tagId: number, name: string) => invoke<void>('rename_tag', { tagId, name }),
  setTagColour: (tagId: number, colour: string) =>
    invoke<void>('set_tag_colour', { tagId, colour }),
  deleteTag: (tagId: number) => invoke<void>('delete_tag', { tagId }),
  // There is deliberately no direct `send` here. Every message leaves through
  // the outbox — saved, scheduled, sent by the worker — so the undo window,
  // the retry ladder and the ambiguous-outcome rule apply to all of them.
  // A binding that sent straight to the wire would be a way around all three.
  storage: () => invoke<StorageReport>('storage_report'),
  exportMbox: (view: string, path: string) => invoke<string>('export_mbox', { view, path }),
  identity: () => invoke<Identity>('get_identity'),
  setIdentity: (displayName: string, signature: string, signatureOnReply: boolean) =>
    invoke<void>('set_identity', { displayName, signature, signatureOnReply }),
  // The whole message, not only its text. A draft that drops its cc, its
  // reply headers or its attachments is fine as long as a draft is only ever
  // a draft; once every send waits in the outbox, it is the message.
  saveDraft: (
    draftId: number | null,
    to: string,
    subject: string,
    body: string,
    html: string,
    rest: { cc?: string; inReplyTo?: string | null; references?: string[]; attachments?: string[] } = {},
  ) =>
    invoke<number>('save_draft', {
      draftId,
      to,
      cc: rest.cc ?? '',
      subject,
      body,
      html,
      inReplyTo: rest.inReplyTo ?? null,
      references: rest.references ?? [],
      attachments: rest.attachments ?? [],
    }),
  loadDraft: (id: number) => invoke<DraftRecord>('load_draft', { id }),
  deleteDraft: (id: number) => invoke<void>('delete_draft', { id }),
  scheduleSend: (draftId: number, atMs: number | null) =>
    invoke<void>('schedule_send', { draftId, atMs }),
  popoutCompose: (draftId: number) => invoke<void>('popout_compose', { draftId }),
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
  search: (query: string, newest = false) =>
    invoke<Thread[]>('search_messages', { query, newest }),

  messageUrl: (messageId: number) => invoke<string>('message_url', { messageId }),
  log: (entry: string) => invoke('frontend_log', { entry }).catch(() => {}),
};

export const api = import.meta.env.DEV && !inTauri() ? mock : real;
