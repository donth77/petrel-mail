/* User-facing strings.
 *
 * Components never hold string literals (AGENTS.md). This module is the single
 * lookup they go through, shaped like the Fluent API it will become: ids are
 * stable and English-ish but not derived from the copy, so rewording a string
 * does not orphan its translations.
 *
 * Deliberately not @fluent/react yet — with one locale that would be ceremony.
 * What matters now is that the *call sites* are right, because those are what is
 * expensive to retrofit; swapping this file's backing store for a real .ftl
 * bundle touches this file only. See docs 07 §13.
 */

type Args = Record<string, string | number>;

const en = {
  'app-name': 'Petrel',

  'rail-mailboxes': 'Mailboxes',
  'rail-switch-account': 'Switch account',
  'mailbox-inbox': 'Inbox',
  'mailbox-starred': 'Starred',
  'mailbox-snoozed': 'Snoozed',
  'mailbox-sent': 'Sent',
  'mailbox-drafts': 'Drafts',
  'mailbox-outbox': 'Outbox',
  'mailbox-archive': 'Archive',
  'mailbox-spam': 'Spam',
  'mailbox-trash': 'Trash',

  'search-placeholder': 'Search this account…',
  'list-conversations': '{ $count } conversations',
  'list-unread': '{ $count } unread',

  'reader-archive': 'Archive',
  'reader-star': 'Star',
  'reader-more': 'More actions',
  'reader-message-count': '{ $count } messages',
  'list-inbox-heading': 'Inbox',
  'search-hint-key': '/',
  'rail-tags': 'Tags',
  'rail-help': 'Help & Shortcuts',
  'rail-settings': 'Settings',
  'titlebar-sync': 'all mail synced',
  'reader-snooze': 'Snooze',
  'reader-reply': 'Reply',
  'reader-reply-all': 'Reply all',
  'reader-forward': 'Forward',
  'reader-failed': 'Could not open this conversation',
  'reader-to': 'to { $who }',
  'reader-earlier': '{ $count } earlier messages',
  'reader-none-title': 'Nothing selected',
  'reader-none-body': 'Pick a conversation with J and K, or press Enter to open one.',

  'empty-inbox-title': 'Inbox is clear',
  'empty-inbox-body': 'Nothing left to triage. Snoozed mail comes back when you asked it to.',
  'empty-search-title': 'No matches for “{ $query }”',
  'empty-search-body': 'All { $count } messages were searched.',
  'empty-loading': 'Loading your mail…',

  'status-synced': 'Synced just now',
  'status-seeding': 'Building your mailbox…',
  'status-counts': '{ $count } conversations · { $unread } unread',

  'a11y-message-list': 'Conversations',
  'a11y-row': '{ $unread }{ $from }, { $subject }, { $time }',
  'a11y-unread-prefix': 'unread, ',
} as const;

export type StringId = keyof typeof en;

/** Fluent-style interpolation: `{ $name }` holes, filled positionally by key. */
export function t(id: StringId, args?: Args): string {
  const raw: string = en[id];
  if (!args) return raw;
  return raw.replace(/\{\s*\$(\w+)\s*\}/g, (whole, key: string) =>
    key in args ? String(args[key]) : whole,
  );
}
