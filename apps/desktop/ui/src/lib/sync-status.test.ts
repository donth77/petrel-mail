import { describe, expect, it } from 'vitest';
import { syncState } from './sync-status';
import type { Status } from './api';

const status = (over: Partial<Status>): Status =>
  ({
    configured: true, demo: false, seeding: false, count: 0, server_total: 0,
    source: '', retention: '', data_dir: '', last_sync_ms: 0, notify: [],
    ...over,
  }) as Status;

describe('syncState', () => {
  it('reports demo mail as demo, not as synced or waiting', () => {
    // The bug this exists to prevent: the title bar said "all mail synced"
    // while the footer said "Waiting to sync…", about the same mailbox.
    expect(syncState(status({ demo: true }))).toEqual({ kind: 'demo' });
  });

  it('does not claim a sync before one has happened', () => {
    expect(syncState(status({ last_sync_ms: 0 }))).toEqual({ kind: 'never' });
  });

  it('reports a failure even when an earlier sync succeeded', () => {
    // A stale success is not the headline; the breakage is.
    expect(
      syncState(status({ sync_error: 'could not sign in', last_sync_ms: 123 })),
    ).toEqual({ kind: 'failed' });
  });

  it('puts seeding ahead of everything, since it is still in motion', () => {
    expect(
      syncState(status({ seeding: true, demo: true, sync_error: 'x' })),
    ).toEqual({ kind: 'seeding' });
  });

  it('ages a real sync from its timestamp', () => {
    expect(syncState(status({ last_sync_ms: 1_700_000_000_000 }))).toEqual({
      kind: 'synced',
      at: 1_700_000_000_000,
    });
  });

  it('treats a missing status as not yet synced rather than synced', () => {
    expect(syncState(null)).toEqual({ kind: 'never' });
  });
});
