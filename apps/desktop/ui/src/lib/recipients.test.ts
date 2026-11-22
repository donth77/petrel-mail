import { describe, expect, it } from 'vitest';
import {
  addressOf,
  firstUnsendable,
  looksLikeAddress,
  moveRecipient,
  sendable,
  splitRecipients,
} from './recipients';

describe('splitRecipients', () => {
  it('takes the separators people actually type', () => {
    expect(splitRecipients('a@x.com, b@y.com; c@z.com')).toEqual([
      'a@x.com', 'b@y.com', 'c@z.com',
    ]);
  });

  it('drops the empties a trailing separator leaves behind', () => {
    expect(splitRecipients('a@x.com, ,')).toEqual(['a@x.com']);
    expect(splitRecipients('   ')).toEqual([]);
  });
});

describe('looksLikeAddress', () => {
  it('accepts the strange but legal', () => {
    for (const ok of [
      'a@b.co',
      'first.last+tag@sub.example.museum',
      "o'brien@example.ie",
      'me@example.co.uk',
    ]) {
      expect(looksLikeAddress(ok), ok).toBe(true);
    }
  });

  /* The point of the suspect chip is the mistake you cannot see in a
     comma-separated line — so these are the ones that must be caught. */
  it('catches what people actually get wrong', () => {
    for (const bad of [
      'Nadia Okafor',   // a name pasted instead of an address
      'nadia@',         // stopped typing
      '@example.com',   // lost the local part
      'nadia@example',  // no dot in the domain
      'a@b@example.com',
      'nadia @example.com',
      '',
    ]) {
      expect(looksLikeAddress(bad), bad).toBe(false);
    }
  });
});

describe('splitRecipients with quoted names', () => {
  it('keeps a comma inside double quotes as part of the name', () => {
    // The form every other client pastes for a surname-first contact. It
    // used to become two chips, one of them an address of `"Wu`.
    expect(splitRecipients('"Wu, Dana" <dana@example.com>, sam@example.com')).toEqual([
      '"Wu, Dana" <dana@example.com>',
      'sam@example.com',
    ]);
  });

  it('still splits on the separators outside quotes', () => {
    expect(splitRecipients('"A; B" <ab@example.com>; c@example.com')).toEqual([
      '"A; B" <ab@example.com>',
      'c@example.com',
    ]);
  });

  it('does not lose the rest of the field to an unclosed quote', () => {
    expect(splitRecipients('"Dana <dana@example.com>')).toEqual(['"Dana <dana@example.com>']);
    // A quote that never closes protects nothing: the separators after it
    // still separate. It made one red chip of every recipient after it.
    expect(splitRecipients('"Wu, Dana <dana@example.com>, alex@example.com')).toEqual([
      '"Wu',
      'Dana <dana@example.com>',
      'alex@example.com',
    ]);
  });
});

describe('splitRecipients with an unclosed <', () => {
  // Typed and never closed. Every recipient after it was swallowed into one
  // red chip with it: removing the chip removed them too, and Send refused
  // the lot (review r3, R3-5).
  it('lets the recipients after it go free', () => {
    expect(splitRecipients('Dana <dana@example.com, bob@example.com')).toEqual([
      'Dana <dana@example.com',
      'bob@example.com',
    ]);
  });

  it('closes at the next >, not at one after another <', () => {
    expect(splitRecipients('Dana <dana@example.com, Bob <bob@example.com>; cy@example.com')).toEqual([
      'Dana <dana@example.com',
      'Bob <bob@example.com>',
      'cy@example.com',
    ]);
  });

  it('still keeps what a closed one holds together', () => {
    expect(splitRecipients('Dana <dana,wu@example.com>, Bob <bob@example.com')).toEqual([
      'Dana <dana,wu@example.com>',
      'Bob <bob@example.com',
    ]);
    expect(splitRecipients('Dana <"wu,dana"@example.com>, "Lee, Al <al@example.com>')).toEqual([
      'Dana <"wu,dana"@example.com>',
      '"Lee',
      'Al <al@example.com>',
    ]);
  });

  it('marks what it leaves unsendable, so nothing goes', () => {
    expect(firstUnsendable({ to: 'Dana <dana@example.com, bob@example.com', cc: '' })).toBe(
      'Dana <dana@example.com',
    );
  });
});

describe('splitRecipients with a pasted column', () => {
  // A column copied from a spreadsheet, one address per line. It used to come
  // out as one entry, which the field drew as a single red chip and Send
  // refused as "Not an address".
  it('makes one recipient of each line', () => {
    expect(splitRecipients('ann@example.com\nbob@example.com\ncy@example.com\n')).toEqual([
      'ann@example.com', 'bob@example.com', 'cy@example.com',
    ]);
  });

  it('takes Windows line ends and blank lines in its stride', () => {
    expect(splitRecipients('ann@example.com\r\n\r\nbob@example.com\r\n')).toEqual([
      'ann@example.com', 'bob@example.com',
    ]);
  });

  it('mixes lines with the separators people type', () => {
    expect(splitRecipients('ann@example.com, bob@example.com\ncy@example.com; dee@example.com')).toEqual([
      'ann@example.com', 'bob@example.com', 'cy@example.com', 'dee@example.com',
    ]);
  });

  it('keeps a quoted name with a comma whole, a line of its own or not', () => {
    expect(splitRecipients('"Wu, Dana" <dana@example.com>\nsam@example.com')).toEqual([
      '"Wu, Dana" <dana@example.com>',
      'sam@example.com',
    ]);
  });

  it('never splits inside the angle brackets', () => {
    expect(splitRecipients('Dana Wu <dana,wu@example.com>; sam@example.com')).toEqual([
      'Dana Wu <dana,wu@example.com>',
      'sam@example.com',
    ]);
  });

  // The send path's own cases (send.rs, `split_recipients`): the chips on
  // screen and the envelope have to agree on every one.
  it('splits as the send path does', () => {
    expect(splitRecipients('"Wu; Dana" <dana@example.com>; alex@example.com')).toEqual([
      '"Wu; Dana" <dana@example.com>',
      'alex@example.com',
    ]);
    expect(splitRecipients('Dana <"wu,dana"@example.com>, alex@example.com')).toEqual([
      'Dana <"wu,dana"@example.com>',
      'alex@example.com',
    ]);
    expect(splitRecipients('  ann@example.com ,, ;\n bob@example.com , ')).toEqual([
      'ann@example.com',
      'bob@example.com',
    ]);
    expect(splitRecipients('')).toEqual([]);
  });
});

describe('looksLikeAddress with a display name', () => {
  it('judges the address inside the angle brackets', () => {
    expect(looksLikeAddress('Dana Wu <dana@example.com>')).toBe(true);
    expect(looksLikeAddress('"Wu, Dana" <dana@example.com>')).toBe(true);
    expect(looksLikeAddress('Dana Wu <dana@example>')).toBe(false);
    expect(looksLikeAddress('Dana Wu <>')).toBe(false);
  });

  it('reads the address out of an entry', () => {
    expect(addressOf('Dana Wu <dana@example.com>')).toBe('dana@example.com');
    expect(addressOf('dana@example.com')).toBe('dana@example.com');
    expect(addressOf('  dana@example.com  ')).toBe('dana@example.com');
  });
});

describe('sendable', () => {
  it('is the rule the wire applies: an @, and nothing that makes two addresses', () => {
    expect(sendable('sam@example.com')).toBe(true);
    expect(sendable('Dana Wu <dana@example.com>')).toBe(true);
    expect(sendable('"Doe, John" <j@x.example>')).toBe(true);
    expect(sendable('bob@localhost')).toBe(true);
    expect(sendable('dan')).toBe(false);
    expect(sendable('Dana Wu')).toBe(false);
    expect(sendable('<>')).toBe(false);
    expect(sendable('a b@example.com')).toBe(false);
  });
});

describe('firstUnsendable', () => {
  it('names the first entry that would be left off the envelope, in To then Cc', () => {
    expect(firstUnsendable({ to: 'sam@example.com', cc: '' })).toBeNull();
    expect(firstUnsendable({ to: 'sam@example.com, dan', cc: '' })).toBe('dan');
    expect(firstUnsendable({ to: 'sam@example.com', cc: 'Dana Wu' })).toBe('Dana Wu');
    // A trailing comma is not an entry.
    expect(firstUnsendable({ to: 'sam@example.com,', cc: '' })).toBeNull();
  });

  it('lets a column of addresses go, one recipient a line', () => {
    expect(firstUnsendable({ to: 'ann@example.com\nbob@example.com', cc: '', bcc: 'cy@example.com\n' })).toBeNull();
  });
});

describe('moveRecipient', () => {
  it('appends to the other field and removes only that chip', () => {
    // The ones left behind keep their order. The chip joins the end of the
    // field it landed in, rather than being inserted where the pointer was.
    expect(
      moveRecipient(
        { to: 'a@x.com, b@y.com, c@z.com', cc: 'd@w.com', bcc: '' },
        { from: 'to', to: 'cc', index: 1, addr: 'b@y.com' },
      ),
    ).toEqual({ to: 'a@x.com, c@z.com', cc: 'd@w.com, b@y.com', bcc: '' });
  });

  it('moves a quoted comma as one chip', () => {
    const chip = '"Wu, Dana" <dana@example.com>';
    expect(
      moveRecipient(
        { to: `${chip}, sam@example.com`, cc: '', bcc: '' },
        { from: 'to', to: 'cc', index: 0, addr: chip },
      ),
    ).toEqual({ to: 'sam@example.com', cc: chip, bcc: '' });
  });

  it('drops an exact duplicate from the source and does not add it again', () => {
    expect(
      moveRecipient(
        { to: 'a@x.com, b@y.com', cc: 'b@y.com', bcc: '' },
        { from: 'to', to: 'cc', index: 1, addr: 'b@y.com' },
      ),
    ).toEqual({ to: 'a@x.com', cc: 'b@y.com', bcc: '' });
  });

  it('returns null for the same field, a missing index, or a different chip', () => {
    // Null rather than a rebuilt string: the double space is the field as the
    // person left it, and a rejected move must not tidy it.
    const spaced = { to: 'a@x.com,  b@y.com', cc: '', bcc: '' };
    expect(moveRecipient(spaced, { from: 'to', to: 'to', index: 0, addr: 'a@x.com' })).toBeNull();
    expect(moveRecipient(spaced, { from: 'to', to: 'cc', index: 5, addr: 'a@x.com' })).toBeNull();
    expect(moveRecipient(spaced, { from: 'to', to: 'cc', index: -1, addr: 'a@x.com' })).toBeNull();
    expect(moveRecipient(spaced, { from: 'to', to: 'cc', index: 0, addr: 'b@y.com' })).toBeNull();
    expect(spaced).toEqual({ to: 'a@x.com,  b@y.com', cc: '', bcc: '' });
  });

  it('leaves an empty string when the last chip moves', () => {
    expect(
      moveRecipient(
        { to: 'a@x.com', cc: '', bcc: '' },
        { from: 'to', to: 'cc', index: 0, addr: 'a@x.com' },
      ),
    ).toEqual({ to: '', cc: 'a@x.com', bcc: '' });
  });

  it('can land in an empty bcc', () => {
    expect(
      moveRecipient(
        { to: 'a@x.com', cc: 'b@y.com', bcc: '' },
        { from: 'to', to: 'bcc', index: 0, addr: 'a@x.com' },
      ),
    ).toEqual({ to: '', cc: 'b@y.com', bcc: 'a@x.com' });
  });

  it('moves a suspicious string', () => {
    // The dashed chip is still a confirmed recipient. Looking unlike an
    // address only paints it; it does not pin it to the field.
    expect(looksLikeAddress('Nadia Okafor')).toBe(false);
    expect(
      moveRecipient(
        { to: 'Nadia Okafor, a@x.com', cc: '', bcc: '' },
        { from: 'to', to: 'cc', index: 0, addr: 'Nadia Okafor' },
      ),
    ).toEqual({ to: 'a@x.com', cc: 'Nadia Okafor', bcc: '' });
  });
});