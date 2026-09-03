import type { ThreadMessage } from './api';

/**
 * The headers that make a reply part of its conversation.
 *
 * Other clients thread by these, not by the subject: without them a reply
 * from here lands in Apple Mail, Outlook and Thunderbird as a new
 * conversation. In-Reply-To names the message answered; References is its
 * own chain with that message appended, so a client that only reads the
 * last entry and one that walks the whole list both find the thread.
 *
 * Bare ids throughout. The sender wraps each in angle brackets exactly once,
 * so wrapping here would double them.
 */
export function replyHeaders(message: Pick<ThreadMessage, 'msgid' | 'references'>): {
  inReplyTo: string | null;
  references: string[];
} {
  const own = message.msgid?.trim() || null;
  const chain = (message.references ?? []).map((r) => r.trim()).filter(Boolean);
  const references = own && !chain.includes(own) ? [...chain, own] : chain;
  return { inReplyTo: own, references };
}

/**
 * Who a reply goes to.
 *
 * Reply-all is the one people get wrong in both directions: leaving someone
 * off a thread they were part of, or writing back to a mailing list that
 * everybody is on. The rules here are the conventional ones.
 *
 * Your own address never appears. Nothing is more obviously broken than a
 * client that copies you on your own reply, and on a long thread it compounds.
 * Duplicates are removed for the same reason, case-insensitively, because
 * addresses differ in case and mean the same person.
 */
export function replyTargets(
  message: ThreadMessage,
  self: string,
  all: boolean,
): { to: string[]; cc: string[] } {
  const mine = self.trim().toLowerCase();
  const seen = new Set<string>([mine]);
  const take = (addr: string) => {
    const key = addr.trim().toLowerCase();
    if (!key || seen.has(key)) return null;
    seen.add(key);
    return addr.trim();
  };

  // The sender is the reply's recipient, whether or not it is a reply-all —
  // unless the sender is you. Replying to something you wrote means writing
  // to the people you wrote to, which is what following up on your own
  // message is; addressing yourself gave an empty To and a send that could
  // not go anywhere.
  const own = message.from_addr.trim().toLowerCase() === mine;
  const to = (own ? (message.recipient_addrs ?? []) : [message.from_addr])
    .map(take)
    .filter((a): a is string => a !== null);
  if (!all) return { to, cc: [] };

  const cc = (message.recipient_addrs ?? [])
    .map(take)
    .filter((a): a is string => a !== null);
  return { to, cc };
}
