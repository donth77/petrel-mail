import { describe, expect, it } from 'vitest';
import { asWords, chips, hasToken, scopedQuery, toggleToken, tokensOf } from './search-chips';

describe('tokensOf', () => {
  it('keeps a quoted value whole', () => {
    expect(tokensOf('from:"Dana Wu" annex')).toEqual(['from:Dana Wu', 'annex']);
  });
});

describe('toggleToken', () => {
  it('adds a token to what is already typed', () => {
    expect(toggleToken('annex', 'has:attachment')).toBe('annex has:attachment');
  });

  it('takes it away again', () => {
    expect(toggleToken('annex has:attachment', 'has:attachment')).toBe('annex');
  });

  it('adds to an empty field', () => {
    expect(toggleToken('', 'is:unread')).toBe('is:unread');
  });

  /* Two senders is a query that matches nothing, and nobody means it. */
  it('replaces a different value for the same operator', () => {
    expect(toggleToken('from:sam annex', 'from:dana')).toBe('annex from:dana');
  });

  it('leaves the words alone either way', () => {
    const on = toggleToken('quarterly report', 'is:starred');
    expect(on).toContain('quarterly');
    expect(on).toContain('report');
    expect(toggleToken(on, 'is:starred')).toBe('quarterly report');
  });

  /* `is:` conditions are separate booleans in the engine, so they narrow
     together — unlike from:/in:/after:, where a second token overwrites the
     first. Treating them all as one-value operators made a chip in the
     Snoozed view throw the view away. */
  it('keeps the states that narrow together', () => {
    expect(toggleToken('is:snoozed', 'is:unread')).toBe('is:snoozed is:unread');
    expect(toggleToken('is:starred', 'is:unread')).toBe('is:starred is:unread');
    expect(toggleToken('is:unread', 'has:attachment')).toBe('is:unread has:attachment');
  });

  it('never drops the scope the search started from', () => {
    expect(toggleToken('in:Receipts', 'is:unread')).toBe('in:Receipts is:unread');
    expect(toggleToken('in:Receipts', 'has:attachment')).toBe('in:Receipts has:attachment');
    expect(toggleToken('is:snoozed', 'has:attachment')).toBe('is:snoozed has:attachment');
  });

  it('still replaces the operators the engine reads as one value', () => {
    expect(toggleToken('in:inbox', 'in:sent')).toBe('in:sent');
    expect(toggleToken('after:2025 annex', 'after:2026')).toBe('annex after:2026');
  });
});

describe('hasToken', () => {
  it('lights a chip from the field, not from a state of its own', () => {
    expect(hasToken('annex has:attachment', 'has:attachment')).toBe(true);
    expect(hasToken('annex', 'has:attachment')).toBe(false);
  });

  /* Typing the operator by hand must light the chip too — otherwise the two
     halves of the same control disagree about what is being searched. */
  it('recognises an operator that was typed rather than clicked', () => {
    expect(hasToken('is:unread', 'is:unread')).toBe(true);
    expect(hasToken('IS:UNREAD', 'is:unread')).toBe(true);
  });

  it('does not mistake a longer token for this one', () => {
    expect(hasToken('has:attachments', 'has:attachment')).toBe(false);
  });
});

describe('chips', () => {
  it('offers the sender only when there is one', () => {
    expect(chips(null, 2026, 'inbox').some((c) => c.id === 'from')).toBe(false);
    expect(chips('Sam Ortiz', 2026, 'inbox').find((c) => c.id === 'from')?.token)
      .toBe('from:"Sam Ortiz"');
  });

  it('quotes a name with a space and leaves a single word bare', () => {
    expect(chips('sam', 2026, 'inbox').find((c) => c.id === 'from')?.token).toBe('from:sam');
  });
});

describe('the scope chip', () => {
  const scope = (view: string) => chips(null, 2026, view).find((c) => c.id === 'scope');

  it('names the mailbox you are actually in', () => {
    expect(scope('inbox')).toEqual({ id: 'scope', label: 'In Inbox', token: 'in:inbox' });
    expect(scope('archive')).toEqual({ id: 'scope', label: 'In Archive', token: 'in:archive' });
    expect(scope('sent')?.token).toBe('in:sent');
  });

  it('offers the way into spam and trash, which search otherwise leaves out', () => {
    expect(scope('spam')?.token).toBe('in:spam');
    expect(scope('trash')?.token).toBe('in:trash');
  });

  it('speaks is: for the state views and stays silent only where it must', () => {
    expect(scope('starred')?.token).toBe('is:starred');
    expect(scope('snoozed')?.token).toBe('is:snoozed');
    expect(scope('outbox')).toBeUndefined();
    expect(scope('tag:Urgent')).toBeUndefined();
  });

  it('does not double the starred chip when the scope already is it', () => {
    const ids = chips(null, 2026, 'starred').map((c) => c.id);
    expect(ids.filter((i) => i === 'starred' || i === 'scope')).toEqual(['scope']);
  });

  it('comes first: the pre-applied context leads the row', () => {
    const ids = chips('Sam', 2026, 'sent').map((c) => c.id);
    expect(ids[0]).toBe('scope');
    expect(ids).toContain('from');
  });

  it('names a user folder by its leaf, quoted when it has spaces', () => {
    expect(chips(null, 2026, 'folder:7', 'Receipts')[0].token).toBe('in:Receipts');
    expect(chips(null, 2026, 'folder:9', 'Client contact')[0].token).toBe('in:"Client contact"');
  });
});

/* What is on gathers at the left. A row that reads lit, unlit, lit, unlit
   makes the reader scan for the answer to "what am I filtering by"; grouped,
   the answer is the first run of chips. */
describe('the order applied chips take', () => {
  it('puts what is on before what is off', () => {
    const ids = chips(null, 2026, 'inbox', null, 'in:inbox is:starred').map((c) => c.id);
    expect(ids.slice(0, 2)).toEqual(['scope', 'starred']);
  });

  it('lifts a chip out of the middle rather than leaving a gap', () => {
    // `unread` sits third in the built row; applied, it comes second.
    const ids = chips(null, 2026, 'inbox', null, 'in:inbox is:unread').map((c) => c.id);
    expect(ids).toEqual(['scope', 'unread', 'attachment', 'starred', 'year']);
  });

  it('keeps the built order inside each group', () => {
    const ids = chips('Sam', 2026, 'sent', null, 'has:attachment after:2026').map((c) => c.id);
    // Applied, in the order they were built; then the rest, likewise.
    expect(ids).toEqual(['attachment', 'year', 'scope', 'from', 'unread', 'starred']);
  });

  it('leads with an applied chip even when the scope was deleted', () => {
    // Deleting the scope token is how a search goes global; the row must not
    // keep a dark chip at the front while a lit one sits behind it.
    const ids = chips(null, 2026, 'inbox', null, 'is:unread').map((c) => c.id);
    expect(ids[0]).toBe('unread');
  });

  it('changes nothing while the row is empty', () => {
    const ids = chips('Sam', 2026, 'sent', null, '').map((c) => c.id);
    expect(ids).toEqual(['scope', 'from', 'attachment', 'unread', 'starred', 'year']);
  });
});

describe('scopedQuery', () => {
  it('scopes a beginning search to wherever you stand', () => {
    expect(scopedQuery('a', '', 'in:spam')).toBe('in:spam a');
    expect(scopedQuery('a', '', 'in:inbox')).toBe('in:inbox a');
    expect(scopedQuery('a', '', 'in:"Client contact"')).toBe('in:"Client contact" a');
  });

  it('never re-applies mid-edit, so deleting the token widens', () => {
    expect(scopedQuery('in:inbox a', 'in:inbox ', 'in:inbox')).toBe('in:inbox a');
    expect(scopedQuery('a', 'in:inbox a', 'in:inbox')).toBe('a');
    expect(scopedQuery('a', '', null)).toBe('a');
  });

  it('only writes the token as the search begins', () => {
    // Otherwise deleting it would fight the person deleting it.
    expect(scopedQuery('in:spam refun', 'in:spam refund', 'spam')).toBe('in:spam refun');
    expect(scopedQuery('refund', 'refun', 'spam')).toBe('refund');
  });

  it('does nothing when the field is being cleared', () => {
    expect(scopedQuery('', 'refund', 'spam')).toBe('');
    expect(scopedQuery('   ', '', 'spam')).toBe('   ');
  });
});

/* A filter in the query must always have a pill. Without one there is no way
   to see what is narrowing the list, and no way to take it off. */
describe('a filter that is applied always has its chip', () => {
  it('keeps the From chip when the search changed what is open', () => {
    // The From chip was built from the open conversation, so running the
    // search — which empties the selection — took the pill away while
    // `from:Slack` went on filtering.
    const ids = chips(null, 2026, 'inbox', null, 'in:inbox from:Slack is:unread').map((c) => c.id);
    expect(ids).toContain('from');
  });

  it('names that chip after the query, not after whatever is open now', () => {
    const from = chips('Someone Else', 2026, 'inbox', null, 'from:Slack').find(
      (c) => c.id === 'from',
    );
    expect(from?.token).toBe('from:Slack');
    expect(from?.label).toBe('From Slack');
  });

  it('lights a chip whose value is quoted', () => {
    expect(hasToken('in:"Client contact" is:unread', 'in:"Client contact"')).toBe(true);
    expect(hasToken('from:"Dana Wu"', 'from:"Dana Wu"')).toBe(true);
  });

  it('does not lose the quotes when another chip is toggled', () => {
    // Splitting the query and joining it back turned `in:"Client contact"`
    // into two words, and the search then meant something else entirely.
    expect(toggleToken('in:"Client contact"', 'is:unread')).toBe(
      'in:"Client contact" is:unread',
    );
  });

  it('can take a quoted chip off again', () => {
    expect(toggleToken('in:"Client contact" is:unread', 'in:"Client contact"')).toBe('is:unread');
  });

  it('names the scope chip after the mailbox being searched, not the one on screen', () => {
    // Standing in the Inbox with `in:Receipts` typed, the row used to offer
    // "In Inbox" unlit while `in:Receipts` narrowed the list with no pill.
    const scope = chips(null, 2026, 'inbox', null, 'in:Receipts is:unread').find(
      (c) => c.id === 'scope',
    );
    expect(scope?.token).toBe('in:Receipts');
    expect(scope?.label).toBe('In Receipts');
  });

  it('still offers the open mailbox when nothing scopes the query', () => {
    const scope = chips(null, 2026, 'inbox', null, 'is:unread').find((c) => c.id === 'scope');
    expect(scope?.token).toBe('in:inbox');
    expect(scope?.label).toBe('In Inbox');
  });

  it('keeps the friendly name when the query scopes where you already are', () => {
    const scope = chips(null, 2026, 'inbox', null, 'in:inbox').find((c) => c.id === 'scope');
    expect(scope?.label).toBe('In Inbox');
  });
});

/* The grammar has phrases, exclusion and OR. A chip click rebuilds the field,
   and a rebuild that reads the pieces bare and guesses the quotes back would
   change what the query means. Every piece goes back as it was typed. */
describe('what was typed survives a chip', () => {
  it('keeps a phrase a phrase', () => {
    expect(toggleToken('"board pack" annex', 'is:unread')).toBe('"board pack" annex is:unread');
    // One word in quotes is still in quotes: it is the word OR, not the operator.
    expect(toggleToken('"OR" theatre', 'is:unread')).toBe('"OR" theatre is:unread');
    // And words that spell an operator stay words.
    expect(toggleToken('"from:sam"', 'is:unread')).toBe('"from:sam" is:unread');
  });

  it('keeps an exclusion outside its quotes', () => {
    // Rebuilt from bare pieces this came back as "-board pack": a phrase that
    // asks for exactly what it was excluding.
    expect(toggleToken('contract -"board pack"', 'has:attachment')).toBe(
      'contract -"board pack" has:attachment',
    );
    expect(toggleToken('-from:"Dana Wu" annex', 'is:unread')).toBe(
      '-from:"Dana Wu" annex is:unread',
    );
  });

  it('keeps curly quotes', () => {
    expect(toggleToken('“board pack”', 'is:unread')).toBe(
      '“board pack” is:unread',
    );
  });

  /* The engine runs an open quote or bracket to the end of the field, so a
     token written after one landed inside it: the chip never lit, and every
     click added another copy. */
  it('closes a quote or a bracket still being typed before writing after it', () => {
    expect(toggleToken('annex "board pa', 'is:unread')).toBe('annex "board pa" is:unread');
    expect(toggleToken('annex “board pa', 'is:unread')).toBe('annex “board pa” is:unread');
    expect(toggleToken('(from:sam OR from:dana', 'is:unread')).toBe(
      '(from:sam OR from:dana) is:unread',
    );
    expect(toggleToken('a OR "b c', 'is:unread')).toBe('(a OR "b c") is:unread');
    for (const typing of ['annex "board pa', '(from:sam OR from:dana', '(']) {
      const once = toggleToken(typing, 'is:unread');
      expect(hasToken(once, 'is:unread')).toBe(true);
      expect(hasToken(toggleToken(once, 'is:unread'), 'is:unread')).toBe(false);
    }
  });

  /* `NOT is:unread` asks for the opposite of the chip. Lit for it, the chip
     took the token out and left the NOT to exclude whatever stood next. */
  it('does not take a NOT for the filter it excludes', () => {
    expect(hasToken('NOT is:unread from:sam', 'is:unread')).toBe(false);
    expect(hasToken('NOT NOT is:unread from:sam', 'is:unread')).toBe(true);
    expect(toggleToken('NOT is:unread from:sam', 'is:unread')).toBe('from:sam is:unread');
    expect(toggleToken('NOT has:attachment invoice', 'has:attachment')).toBe(
      'invoice has:attachment',
    );
    // Taken off, it takes its NOTs with it.
    expect(toggleToken('NOT NOT is:unread from:sam', 'is:unread')).toBe('from:sam');
    // An excluded mailbox is neither replaced nor the one being searched.
    expect(toggleToken('NOT in:spam x', 'in:inbox')).toBe('NOT in:spam x in:inbox');
    const scope = chips(null, 2026, 'inbox', null, 'NOT in:spam x').find((c) => c.id === 'scope');
    expect(scope?.token).toBe('in:inbox');
    // A NOT still waiting for its word does not get the chip's token.
    expect(toggleToken('invoice NOT', 'is:unread')).toBe('invoice is:unread NOT');
    // However many dashes, one exclusion.
    expect(toggleToken('x --is:unread', 'is:unread')).toBe('x is:unread');
  });

  it('does not take a phrase for the operator it spells', () => {
    expect(hasToken('"is:unread"', 'is:unread')).toBe(false);
    expect(toggleToken('"is:unread"', 'is:unread')).toBe('"is:unread" is:unread');
    const scope = chips(null, 2026, 'inbox', null, '"in:receipts and more"').find(
      (c) => c.id === 'scope',
    );
    expect(scope?.token).toBe('in:inbox');
  });

  it('does not take an exclusion for the filter it excludes', () => {
    expect(hasToken('-is:unread', 'is:unread')).toBe(false);
    // Asking for unread replaces asking for not-unread; both at once is nothing.
    expect(toggleToken('annex -is:unread', 'is:unread')).toBe('annex is:unread');
    // An excluded mailbox is not the mailbox being searched.
    const scope = chips(null, 2026, 'inbox', null, '-in:spam annex').find((c) => c.id === 'scope');
    expect(scope?.token).toBe('in:inbox');
  });
});

/* OR binds looser than everything else, so a filter written once after an OR
   would hold for the last alternative only. The query goes into brackets. */
describe('a chip narrows the whole query', () => {
  it('brackets an OR before it writes itself in', () => {
    expect(toggleToken('from:sam OR from:dana', 'is:unread')).toBe(
      '(from:sam OR from:dana) is:unread',
    );
  });

  it('comes off again and takes its brackets with it', () => {
    expect(toggleToken('(from:sam OR from:dana) is:unread', 'is:unread')).toBe(
      'from:sam OR from:dana',
    );
    // Brackets that were doing something stay.
    expect(toggleToken('(from:sam OR from:dana) annex is:unread', 'is:unread')).toBe(
      '(from:sam OR from:dana) annex',
    );
  });

  it('is lit when it stands outside the brackets, or on every side of an OR', () => {
    expect(hasToken('(from:sam OR from:dana) is:unread', 'is:unread')).toBe(true);
    expect(hasToken('from:sam is:unread OR from:dana is:unread', 'is:unread')).toBe(true);
    expect(hasToken('from:sam is:unread OR from:dana', 'is:unread')).toBe(false);
    // Inside brackets it is one side of a choice, not a filter on the result.
    expect(hasToken('(is:unread OR is:starred) annex', 'is:unread')).toBe(false);
  });

  it('comes off every side when that is where it was typed', () => {
    expect(toggleToken('from:sam is:unread OR from:dana is:unread', 'is:unread')).toBe(
      'from:sam OR from:dana',
    );
  });

  it('leaves a lowercase or alone: it is a word', () => {
    expect(toggleToken('now or never', 'is:unread')).toBe('now or never is:unread');
  });

  it('keeps an OR that is still waiting for its other side', () => {
    expect(toggleToken('from:sam OR', 'is:unread')).toBe('from:sam is:unread OR');
    expect(toggleToken('OR', 'is:unread')).toBe('is:unread');
  });

  it('reads the scope and the sender from outside the brackets only', () => {
    const row = chips(null, 2026, 'inbox', null, '(in:receipts OR in:archive) annex');
    expect(row.find((c) => c.id === 'scope')?.token).toBe('in:inbox');
    expect(row.some((c) => c.id === 'from')).toBe(false);
  });

  it('puts back brackets, AND and NOT exactly as they were typed', () => {
    expect(toggleToken('invoice AND NOT (draft OR wip)', 'has:attachment')).toBe(
      'invoice AND NOT (draft OR wip) has:attachment',
    );
    expect(toggleToken('contract -(draft OR "work in progress")', 'is:unread')).toBe(
      'contract -(draft OR "work in progress") is:unread',
    );
  });
});

/* AND, OR and NOT in capitals are operators, always: the engine does not
   guess. What the field does is notice when the capitals do not stand out —
   a pasted subject line — and offer the quoted reading in one click. */
describe('a keyword that may have been meant as a word', () => {
  const suggestion = (query: string) =>
    chips(null, 2026, 'inbox', null, query).find((c) => c.rewrite !== undefined);

  /* The phrase, not the keyword alone in quotes: somebody who pasted a subject
     line is looking for that line, and `TERMS "AND" CONDITIONS` would find
     any mail with the three words anywhere in it. */
  it('is offered as the phrase it sits in when the words beside it are in capitals too', () => {
    expect(asWords('TERMS AND CONDITIONS')).toEqual({
      words: 'TERMS AND CONDITIONS',
      rewrite: '"TERMS AND CONDITIONS"',
    });
    expect(asWords('DO NOT REPLY')?.rewrite).toBe('"DO NOT REPLY"');
    expect(asWords('Please DO NOT reply')?.rewrite).toBe('"Please DO NOT reply"');
    expect(asWords('IBM OR HP')?.rewrite).toBe('"IBM OR HP"');
    expect(asWords('READ AND SIGN OR RETURN AND KEEP')?.rewrite).toBe(
      '"READ AND SIGN OR RETURN AND KEEP"',
    );
  });

  it('ends the phrase at an operator, a bracket, an exclusion or a quote', () => {
    // The scope the field writes for itself stays a scope.
    expect(asWords('in:inbox DO NOT REPLY')?.rewrite).toBe('in:inbox "DO NOT REPLY"');
    expect(asWords('DO NOT REPLY is:unread')?.rewrite).toBe('"DO NOT REPLY" is:unread');
    expect(asWords('from:sam TERMS AND CONDITIONS -draft')?.rewrite).toBe(
      'from:sam "TERMS AND CONDITIONS" -draft',
    );
    expect(asWords('(TERMS AND CONDITIONS) invoice')?.rewrite).toBeUndefined();
    expect(asWords('"signed" TERMS AND CONDITIONS')?.rewrite).toBe(
      '"signed" "TERMS AND CONDITIONS"',
    );
  });

  it('is left alone when it is the only thing in capitals', () => {
    expect(asWords('invoice NOT draft')).toBeNull();
    expect(asWords('annex pricing')).toBeNull();
    expect(asWords('from:sam OR from:dana')).toBeNull();
    expect(asWords('from:SAM OR from:DANA')).toBeNull();
    expect(asWords('(from:sam OR from:dana) AND is:unread')).toBeNull();
    expect(asWords('東京 OR 大阪')).toBeNull();
    expect(asWords('2024 OR 2025')).toBeNull();
    // Already in quotes: already a word.
    expect(asWords('DO "NOT" REPLY')).toBeNull();
    // A phrase has a word at each end. These have a keyword at one, and
    // `"AND BAR"` is nothing anybody pasted.
    expect(asWords('(FOO) AND BAR')).toBeNull();
    expect(asWords('-FOO AND BAR')).toBeNull();
    expect(asWords('FOO AND -BAR')).toBeNull();
    expect(asWords('"TERMS" AND CONDITIONS')).toBeNull();
    // `I` and `A` are capitals by spelling, not by shouting.
    expect(asWords('A OR B')).toBeNull();
    expect(asWords('I OR you')).toBeNull();
    // The run keeps its words and leaves a keyword at its end where it was.
    expect(asWords('DO NOT REPLY OR')?.rewrite).toBe('"DO NOT REPLY" OR');
  });

  it('is a chip at the end of the row, never lit, that swaps the query', () => {
    const row = chips(null, 2026, 'inbox', null, 'in:inbox TERMS AND CONDITIONS');
    const chip = row[row.length - 1];
    expect(chip.id).toBe('as-words');
    expect(chip.label).toBe('Search for “TERMS AND CONDITIONS”');
    expect(chip.rewrite).toBe('in:inbox "TERMS AND CONDITIONS"');
    expect(hasToken('in:inbox TERMS AND CONDITIONS', chip.token)).toBe(false);
    // Taking it settles the matter: nothing left to suggest.
    expect(suggestion(chip.rewrite!)).toBeUndefined();
  });

  it('still reads the OR as the operator until then', () => {
    expect(toggleToken('DEAD OR ALIVE', 'is:unread')).toBe('(DEAD OR ALIVE) is:unread');
  });
});
