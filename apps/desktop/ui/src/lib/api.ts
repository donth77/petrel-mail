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

export type Status = {
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

const mock = {
  status: async (): Promise<Status> => ({
    seeding: false, count: 10000, source: 'tom@northbay.example',
    retention: 'mirror', data_dir: '~/Library/Application Support/Petrel',
  }),
  threads: async (offset: number, limit: number) => mockRows(Math.min(limit, 2000), offset),
  search: async (q: string) =>
    mockRows(24).filter((r) => r.subject.toLowerCase().includes(q.toLowerCase())),
  messageUrl: async () => '',
  log: async () => {},
};

const real = {
  status: () => invoke<Status>('status'),
  threads: (offset: number, limit: number) =>
    invoke<Thread[]>('list_threads', { offset, limit }),
  search: (query: string) => invoke<Thread[]>('search_messages', { query }),

  messageUrl: (messageId: number) => invoke<string>('message_url', { messageId }),
  log: (entry: string) => invoke('frontend_log', { entry }).catch(() => {}),
};

export const api = import.meta.env.DEV && !inTauri() ? mock : real;
