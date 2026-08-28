import { describe, expect, it } from 'vitest';
import { countDeltas, type CountMode } from './counts';

/** The common case: an unread conversation triaged out of the inbox. */
const from = (
  kind: Parameters<typeof countDeltas>[0]['kind'],
  over: Partial<Parameters<typeof countDeltas>[0]> = {},
) =>
  countDeltas({
    kind,
    view: 'inbox',
    unread: true,
    removes: true,
    mode: 'unread' as CountMode,
    ...over,
  });

describe('countDeltas', () => {
  it('moves the conversation out of one number and into the other', () => {
    expect(from('trash')).toEqual({ inbox: -1, trash: 1 });
    expect(from('archive')).toEqual({ inbox: -1, archive: 1 });
    expect(from('spam')).toEqual({ inbox: -1, spam: 1 });
    expect(from('snooze')).toEqual({ inbox: -1, snoozed: 1 });
  });

  it('takes one away and gives it to nothing when it is destroyed', () => {
    expect(from('delete_forever')).toEqual({ inbox: -1 });
  });

  it('says nothing at all when badges are off', () => {
    expect(from('trash', { mode: 'off' })).toEqual({});
  });

  it('leaves a read conversation alone, because the numbers are unread ones', () => {
    expect(from('trash', { unread: false })).toEqual({});
    // Unless they are totals, and then every conversation counts.
    expect(from('trash', { unread: false, mode: 'total' })).toEqual({ inbox: -1, trash: 1 });
  });

  it('does not claim a conversation arrived where it already was', () => {
    // Trashing from the bin: the row does not leave, and nothing arrives.
    expect(from('trash', { view: 'trash', removes: false })).toEqual({});
    expect(from('archive', { view: 'archive', removes: false })).toEqual({});
  });

  it('knows the bin gained one even when it cannot name what lost one', () => {
    // A tag view has no number, so only half of the move is visible here. The
    // recount that follows fills in the rest.
    expect(from('trash', { view: 'tag:Urgent' })).toEqual({ trash: 1 });
    expect(from('trash', { view: 'folder:12' })).toEqual({ trash: 1 });
  });

  it('counts a move only when it names a mailbox that has a number', () => {
    expect(from('move', { view: 'archive', toRole: 'inbox' })).toEqual({ archive: -1, inbox: 1 });
    // Filed into a folder of your own: folders carry no badge.
    expect(from('move', { view: 'archive' })).toEqual({ archive: -1 });
  });

  it('drops the source only when the row actually leaves it', () => {
    expect(from('star', { removes: false })).toEqual({});
    expect(from('unstar', { view: 'starred' })).toEqual({ starred: -1 });
  });

  it('counts Sent and the always-total views by their own rules', () => {
    // Sent has no number in unread mode, so nothing moves.
    expect(from('trash', { view: 'sent', unread: false })).toEqual({});
    expect(from('trash', { view: 'sent', unread: false, mode: 'total' })).toEqual({
      sent: -1,
      trash: 1,
    });
    // Drafts is a total whatever the mode, so a read draft still counts.
    expect(from('trash', { view: 'drafts', unread: false })).toEqual({ drafts: -1 });
  });
});
