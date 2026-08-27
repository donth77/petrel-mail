import type { Status } from './api';

/**
 * What the sync is actually doing, decided once.
 *
 * The title bar and the footer both describe the sync, and they used to decide
 * separately: the footer aged a real timestamp, while the title bar asked only
 * whether seeding had finished and otherwise said "all mail synced". So a demo
 * mailbox — which never syncs and never will — claimed to be synced in one
 * line and to be waiting in the next, a first run said both at once until the
 * first cycle landed, and a failed sync was announced as success directly above
 * its own error banner. The two still word it differently, because the footer
 * has room to age the time and the title bar does not, but they are no longer
 * allowed to describe two different situations.
 *
 * Order matters. Seeding first, because it is the only one still in motion.
 * Then failure, because a sync that broke outranks whatever succeeded before
 * it. Then demo, because "waiting to sync" promises something that is never
 * coming when there is no account to sync with.
 */
export type SyncState =
  | { kind: 'seeding' }
  | { kind: 'failed' }
  | { kind: 'demo' }
  | { kind: 'never' }
  | { kind: 'synced'; at: number };

export function syncState(status: Status | null | undefined): SyncState {
  if (status?.seeding) return { kind: 'seeding' };
  if (status?.sync_error) return { kind: 'failed' };
  if (status?.demo) return { kind: 'demo' };
  if (status?.last_sync_ms) return { kind: 'synced', at: status.last_sync_ms };
  return { kind: 'never' };
}
