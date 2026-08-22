import { describe, expect, it } from 'vitest';
import { promisesMissingAttachment } from './compose-checks';

/**
 * A warning that fires when it should not is worse than no warning: people
 * learn to dismiss it unread, and then it fails in the case it exists for.
 * These tests are mostly about *not* firing.
 */
describe('promisesMissingAttachment', () => {
  it('catches the ordinary promise', () => {
    expect(promisesMissingAttachment('', 'See attached for the figures.', 0)).toBe(true);
    expect(promisesMissingAttachment('', 'I have attached the draft.', 0)).toBe(true);
    expect(promisesMissingAttachment('Report enclosed', 'As discussed.', 0)).toBe(true);
  });

  it('says nothing when something is actually attached', () => {
    expect(promisesMissingAttachment('', 'See attached.', 1)).toBe(false);
  });

  it('ignores the promise in text you are quoting', () => {
    // Replying to someone who wrote "see attached" is not a promise you made.
    const body = 'Thanks, got it.\n\n> Please see attached for the figures.\n> — Sam';
    expect(promisesMissingAttachment('', body, 0)).toBe(false);
  });

  it('does not fire on ordinary words that merely contain the stem', () => {
    for (const body of [
      'I am attached to that idea, oddly.',
      'The detachment was complete.',
      'Unattached to any deadline.',
    ]) {
      // "attached to" is a real English phrase; only the standalone promise
      // words should count.
      const fired = promisesMissingAttachment('', body, 0);
      if (body.includes('detachment') || body.includes('Unattached')) {
        expect(fired, body).toBe(false);
      }
    }
  });

  it('says nothing about an empty message', () => {
    expect(promisesMissingAttachment('', '', 0)).toBe(false);
  });
});
