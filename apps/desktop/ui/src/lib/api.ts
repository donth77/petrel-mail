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
function mockRows(n: number, offset = 0): Listing[] {
  return Array.from({ length: n }, (_, i) => {
    const k = offset + i;
    const [display, addr] = NAMES[k % NAMES.length];
    return {
      id: k + 1,
      from_display: display,
      from_addr: addr,
      subject: `${SUBJECTS[k % SUBJECTS.length]}${k > 5 ? ` (${k})` : ''}`,
      snippet: 'the twelve-month term works, and the volume tier resets annually rather than…',
      date_ms: Date.now() - k * 37 * 60 * 1000,
    };
  });
}

const mock = {
  status: async (): Promise<Status> => ({
    seeding: false, count: 10000, source: 'tom@northbay.example',
    retention: 'mirror', data_dir: '~/Library/Application Support/Petrel',
  }),
  list: async (offset: number, limit: number) => mockRows(Math.min(limit, 2000), offset),
  search: async (q: string) => mockRows(24).filter((r) => r.subject.toLowerCase().includes(q.toLowerCase())),
  messageUrl: async () => '',
  log: async () => {},
};

const real = {
  status: () => invoke<Status>('status'),
  list: (offset: number, limit: number) =>
    invoke<Listing[]>('list_messages', { offset, limit }),
  search: (query: string) => invoke<Listing[]>('search_messages', { query }),
  messageUrl: (messageId: number) => invoke<string>('message_url', { messageId }),
  log: (entry: string) => invoke('frontend_log', { entry }).catch(() => {}),
};

export const api = import.meta.env.DEV && !inTauri() ? mock : real;
