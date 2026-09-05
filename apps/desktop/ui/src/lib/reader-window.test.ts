import { describe, expect, it } from 'vitest';
import {
  MAX_OPEN_BODIES,
  THREAD_PAGE_MAX,
  bodiesToMount,
  clampThreadLimit,
  keepExistingPane,
  nextExpanded,
} from './reader-window';

describe('keepExistingPane', () => {
  it('holds the cards only for the thread already on screen', () => {
    expect(keepExistingPane({ loadedThreadId: 5, requestedThreadId: 5 })).toBe(true);
    expect(keepExistingPane({ loadedThreadId: 5, requestedThreadId: 6 })).toBe(false);
    expect(keepExistingPane({ loadedThreadId: null, requestedThreadId: 5 })).toBe(false);
  });
});

describe('clampThreadLimit', () => {
  it('clamps below one and above the IPC cap', () => {
    expect(clampThreadLimit(0)).toBe(1);
    expect(clampThreadLimit(-5)).toBe(1);
    expect(clampThreadLimit(THREAD_PAGE_MAX + 50)).toBe(THREAD_PAGE_MAX);
  });
});

describe('nextExpanded', () => {
  it('never exceeds the iframe cap', () => {
    const out = nextExpanded({ prev: new Set([1, 2, 3]), add: 4, newestId: 10 });
    expect(out.size).toBeLessThanOrEqual(MAX_OPEN_BODIES);
  });

  it('keeps the newest and the row just opened', () => {
    const out = nextExpanded({ prev: new Set([1, 2, 3]), add: 4, newestId: 3 });
    expect(out.has(4)).toBe(true);
    expect(out.has(3)).toBe(true);
  });

  it('stays capped after many walks', () => {
    let expanded = new Set<number>([10]);
    for (let i = 1; i <= 20; i += 1) {
      expanded = nextExpanded({ prev: expanded, add: i, newestId: 10 });
      expect(expanded.size).toBeLessThanOrEqual(MAX_OPEN_BODIES);
    }
  });
});

describe('bodiesToMount', () => {
  it('mounts at most three bodies', () => {
    const expanded = new Set([1, 2, 3, 4]);
    expect(bodiesToMount(expanded, 4).size).toBeLessThanOrEqual(MAX_OPEN_BODIES);
  });
});
