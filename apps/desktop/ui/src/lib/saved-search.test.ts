import { describe, expect, it } from 'vitest';
import { canSave, savedState, suggestName } from './saved-search';
import type { SavedSearch } from './api';

const pinned = (id: number, query: string): SavedSearch => ({
  id,
  name: `S${id}`,
  query,
  position: id,
});

describe('a name to offer for a query', () => {
  /* The query with its syntax stripped. An earlier rule offered one value from
     the whole query, which called `(from:sam OR from:dana) invoice` "Sam" and
     left a rail of searches sharing one name. */
  it('reads back what was typed, without the punctuation', () => {
    expect(suggestName('invoice from:sam')).toBe('Invoice Sam');
    expect(suggestName('annex pricing')).toBe('Annex pricing');
    expect(suggestName('is:unread has:attachment')).toBe('Unread attachment');
    expect(suggestName('subject:"board pack"')).toBe('Board pack');
  });

  it('keeps an OR, because it is what the query means', () => {
    expect(suggestName('(from:sam OR from:dana) invoice')).toBe('Sam or Dana invoice');
    expect(suggestName('annex OR pricing')).toBe('Annex or pricing');
    // A leading OR is grammar with nothing before it.
    expect(suggestName('OR invoice')).toBe('Invoice');
  });

  it('capitalises a name and leaves a word alone', () => {
    // from:, to:, cc:, in:, tag:, filename: name somebody or somewhere.
    expect(suggestName('from:sam')).toBe('Sam');
    expect(suggestName('tag:waiting')).toBe('Waiting');
    expect(suggestName('from:McBride')).toBe('McBride');
    expect(suggestName('from:élodie')).toBe('Élodie');
    expect(suggestName('東京')).toBe('東京');
    // A word you searched for is not a proper noun, so only the name's first
    // letter rises.
    expect(suggestName('invoice annex')).toBe('Invoice annex');
  });

  it('leaves out what a name cannot carry', () => {
    // An excluded term: "Waiting unread" would say the opposite of the query.
    expect(suggestName('tag:waiting -is:unread')).toBe('Waiting');
    expect(suggestName('NOT draft annex')).toBe('Annex');
    expect(suggestName('-draft')).toBe('');
    // And the punctuation at either edge.
    expect(suggestName('filename:.pdf')).toBe('Pdf');
    expect(suggestName('from:')).toBe('From');
  });

  it('stops at five values, because a name is a label', () => {
    expect(suggestName('one two three four five six seven')).toBe('One two three four five');
  });

  /* The dialog asks with a blank box rather than offering a name made of
     nothing. */
  it('offers nothing when there is nothing to name from', () => {
    for (const query of ['', '   ', ':', '-', '()', '"', '-draft -annex']) {
      expect(suggestName(query), JSON.stringify(query)).toBe('');
    }
  });
});

describe('whether the field still holds the search that is open', () => {
  const searches = [pinned(1, 'tag:waiting -is:unread'), pinned(2, 'invoice')];

  it('says none for a view that is not a saved search', () => {
    expect(savedState('inbox', 'invoice', searches).kind).toBe('none');
    expect(savedState('tag:Urgent', '', searches).kind).toBe('none');
    // And for a saved search that is no longer there — deleted in another
    // window, or belonging to an account that has gone.
    expect(savedState('search:9', 'invoice', searches).kind).toBe('none');
  });

  it('says saved while the text is the stored text', () => {
    const state = savedState('search:1', 'tag:waiting -is:unread', searches);
    expect(state.kind).toBe('saved');
    expect(state.kind !== 'none' && state.search.id).toBe(1);
  });

  it('says edited once a character changes', () => {
    expect(savedState('search:1', 'tag:waiting', searches).kind).toBe('edited');
    expect(savedState('search:1', 'tag:waiting -is:unread annex', searches).kind).toBe('edited');
    expect(savedState('search:2', '', searches).kind).toBe('edited');
  });

  /* Trailing space while somebody types the next word is not an edit worth
     offering to save. */
  it('ignores the edges', () => {
    expect(savedState('search:2', '  invoice  ', searches).kind).toBe('saved');
  });
});

describe('whether the field can be saved', () => {
  const searches = [pinned(1, 'tag:waiting'), pinned(2, 'invoice')];
  const at = (view: string, query: string) => canSave(true, savedState(view, query, searches));

  it('says no while the field holds the mailbox', () => {
    expect(canSave(false, savedState('inbox', '', searches))).toBe(false);
    expect(canSave(false, savedState('search:1', 'tag:waiting', searches))).toBe(false);
  });

  it('says yes for a search that is not pinned', () => {
    expect(at('inbox', 'from:sam')).toBe(true);
  });

  it('says no for a saved search exactly as saved', () => {
    expect(at('search:1', 'tag:waiting')).toBe(false);
    // Edges do not count as an edit.
    expect(at('search:1', '  tag:waiting  ')).toBe(false);
  });

  /* Editing a saved search's text means running a different search, so it can
     be saved as a new one. There is no "update": offering it read as an
     invitation to overwrite the search that had just been opened. */
  it('says yes once the text has changed', () => {
    expect(at('search:1', 'tag:waiting annex')).toBe(true);
    expect(at('search:2', 'receipts')).toBe(true);
  });

  it('says yes when the search it came from is gone', () => {
    expect(at('search:9', 'from:dana')).toBe(true);
  });
});
