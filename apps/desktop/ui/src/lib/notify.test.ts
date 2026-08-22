import { describe, expect, it } from 'vitest';
import { DEFAULTS, type Settings } from './settings';
import { notifiable, shouldNotify } from './notify';
import type { Thread } from './api';

/**
 * The notification rules decide when to interrupt someone, which is the kind of
 * thing that has to be right rather than nearly right: a paused app that still
 * buzzes has broken its only promise.
 */

const settings = (over: Partial<Settings> = {}): Settings => ({ ...DEFAULTS, ...over });

let n = 0;
const thread = (over: Partial<Thread> = {}): Thread =>
  ({
    thread_id: -++n,
    id: n,
    from_display: 'Sam',
    from_addr: 'sam@example.com',
    subject: 'Subject',
    snippet: '',
    date_ms: 0,
    message_count: 1,
    participants: 'Sam',
    unread: true,
    starred: false,
    has_attachments: false,
    tags: [],
    attachment_name: '',
    ...over,
  }) as Thread;

const NOW = 1_000_000;

describe('pause', () => {
  it('silences everything while it is running', () => {
    const s = settings({ notifyPausedUntil: String(NOW + 60_000) });
    expect(shouldNotify(s, NOW)).toBe(false);
    expect(notifiable(s, [thread()], NOW)).toHaveLength(0);
  });

  it('lapses on its own rather than needing to be switched back', () => {
    // Stored as an instant precisely so it cannot be left on by accident.
    const s = settings({ notifyPausedUntil: String(NOW - 1) });
    expect(shouldNotify(s, NOW)).toBe(true);
  });

  it('treats an absent or unparseable value as not paused', () => {
    expect(shouldNotify(settings({ notifyPausedUntil: '0' }), NOW)).toBe(true);
    expect(shouldNotify(settings({ notifyPausedUntil: '' }), NOW)).toBe(true);
    expect(shouldNotify(settings({ notifyPausedUntil: 'nonsense' }), NOW)).toBe(true);
  });
});

describe('level', () => {
  it('announces every unread arrival on "all"', () => {
    const s = settings({ notifyLevel: 'all' });
    expect(notifiable(s, [thread(), thread()], NOW)).toHaveLength(2);
  });

  it('announces only starred arrivals on "priority"', () => {
    const s = settings({ notifyLevel: 'priority' });
    const got = notifiable(s, [thread(), thread({ starred: true })], NOW);
    expect(got).toHaveLength(1);
    expect(got[0].starred).toBe(true);
  });

  it('announces nothing on "none", pause or no pause', () => {
    const s = settings({ notifyLevel: 'none' });
    expect(notifiable(s, [thread({ starred: true })], NOW)).toHaveLength(0);
    expect(shouldNotify(s, NOW)).toBe(false);
  });
});

describe('what counts as an arrival', () => {
  it('ignores mail that arrived already read', () => {
    // Sent from another device, or already seen elsewhere. Announcing it is
    // announcing something the user has demonstrably dealt with.
    const s = settings();
    expect(notifiable(s, [thread({ unread: false })], NOW)).toHaveLength(0);
  });

  it('says nothing when nothing arrived', () => {
    expect(notifiable(settings(), [], NOW)).toHaveLength(0);
  });
});
