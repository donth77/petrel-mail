import { describe, expect, it } from 'vitest';
import { replyTargets } from './reply';
import type { ThreadMessage } from './api';

const message = (over: Partial<ThreadMessage> = {}): ThreadMessage =>
  ({
    id: 1,
    from_display: 'Sam',
    from_addr: 'sam@example.com',
    subject: 'Hi',
    snippet: '',
    date_ms: 0,
    unread: false,
    recipients: [],
    recipient_addrs: ['you@example.com', 'dana@example.com'],
    attachments: [],
    ...over,
  }) as ThreadMessage;

describe('reply', () => {
  it('goes to the sender', () => {
    expect(replyTargets(message(), 'you@example.com', false)).toEqual({
      to: ['sam@example.com'],
      cc: [],
    });
  });

  it('copies nobody', () => {
    expect(replyTargets(message(), 'you@example.com', false).cc).toEqual([]);
  });
});

describe('reply all', () => {
  it('copies everyone else on the thread', () => {
    const { to, cc } = replyTargets(message(), 'you@example.com', true);
    expect(to).toEqual(['sam@example.com']);
    expect(cc).toEqual(['dana@example.com']);
  });

  it('never copies you on your own reply', () => {
    // Nothing looks more broken, and on a long thread it compounds.
    const { to, cc } = replyTargets(message(), 'you@example.com', true);
    expect([...to, ...cc]).not.toContain('you@example.com');
  });

  it('ignores case when deciding that an address is yours', () => {
    const m = message({ recipient_addrs: ['You@Example.com', 'dana@example.com'] });
    expect(replyTargets(m, 'you@example.com', true).cc).toEqual(['dana@example.com']);
  });

  it('does not list the sender twice when they are also a recipient', () => {
    const m = message({ recipient_addrs: ['sam@example.com', 'dana@example.com'] });
    const { to, cc } = replyTargets(m, 'you@example.com', true);
    expect(to).toEqual(['sam@example.com']);
    expect(cc).toEqual(['dana@example.com']);
  });

  it('degrades to a plain reply when there is nobody else', () => {
    const m = message({ recipient_addrs: ['you@example.com'] });
    expect(replyTargets(m, 'you@example.com', true)).toEqual({
      to: ['sam@example.com'],
      cc: [],
    });
  });
});
