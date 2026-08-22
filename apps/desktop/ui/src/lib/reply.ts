import type { ThreadMessage } from './api';

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

  // The sender is the reply's recipient, whether or not it is a reply-all.
  const to = [message.from_addr].map(take).filter((a): a is string => a !== null);
  if (!all) return { to, cc: [] };

  const cc = (message.recipient_addrs ?? [])
    .map(take)
    .filter((a): a is string => a !== null);
  return { to, cc };
}
