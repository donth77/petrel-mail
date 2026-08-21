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

  'cmd-archive': 'Mark Done',
  'cmd-archive-alias': 'Archive',
  'cmd-snooze': 'Snooze this conversation',
  'cmd-star': 'Star',
  'cmd-tag': 'Tag',
  'cmd-move': 'Move to folder',
  'cmd-reply': 'Reply',
  'cmd-trash': 'Move to trash',
  'cmd-compose': 'Compose',
  'cmd-search': 'Search',
  'cmd-pause-notifications': 'Pause notifications for 1 hour',
  'hint-folder': 'folder',
  'hint-settings': 'settings',
  'settings-title': 'Settings',
  'settings-accounts': 'Accounts',
  'settings-identities': 'Identities & Signatures',
  'settings-composing': 'Composing',
  'settings-notifications': 'Notifications',
  'settings-appearance': 'Appearance',
  'settings-privacy': 'Privacy & Security',
  'settings-storage': 'Storage & Data',
  'settings-not-built': 'This pane is not built yet.',
  'accounts-yours': 'Your accounts',
  'accounts-add': 'Add account',
  'accounts-none': 'No accounts yet.',
  'accounts-failed': 'Could not read your accounts',
  'accounts-synced': 'synced { $when }',
  'accounts-storage': '{ $count } messages stored on this Mac.',
  'accounts-colour': 'Colour',
  'accounts-colour-help': 'Marks this account across the app.',
  'accounts-keep': 'When mail disappears from the server',
  'accounts-keep-mirror': 'Gone there, gone here — after a 30-day grace period in case it was a mistake.',
  'accounts-keep-archive': 'Petrel keeps its copy after the server drops it. Your archive outlives the mailbox — and grows without limit.',
  'accounts-mirror': 'Mirror the server',
  'accounts-archive': 'Keep a local archive',
  'accounts-folders': 'Folder mapping',
  'accounts-folders-help': 'Detected from the server (SPECIAL-USE). Change one only if your provider labels things unusually.',
  'accounts-folders-none': 'Nothing mapped yet — this account has not synced its folder list.',
  'folder-archive': 'Archive',
  'folder-sent': 'Sent',
  'folder-drafts': 'Drafts',
  'folder-spam': 'Spam',
  'folder-trash': 'Trash',
  'folder-unmapped': 'not mapped',
  'appearance-theme': 'Theme',
  'appearance-theme-help': 'System follows your operating system, including on a schedule.',
  'theme-light': 'Light',
  'theme-dark': 'Dark',
  'theme-system': 'System',
  'appearance-language': 'Language',
  'appearance-language-help': "System follows your Mac's language. Petrel falls back to English for anything not yet translated.",
  'language-system': 'System — English',
  'appearance-accent': 'Accent',
  'appearance-accent-help': 'Used for selection, unread marks and the one emphasised action on any screen.',
  'appearance-list': 'Message list',
  'appearance-list-help': 'Density affects the list only — the message you are reading keeps its comfortable size.',
  'appearance-density': 'Density',
  'appearance-reading-pane': 'Reading pane',
  'density-relaxed': 'Relaxed',
  'density-compact': 'Compact',
  'layout-right': 'Right',
  'layout-below': 'Below',
  'layout-off': 'Off',
  'appearance-text-size': 'Reading text size',
  'appearance-text-size-help': 'Message bodies only. Interface text follows your system settings.',
  'reset': 'Reset',
  'help-title': 'Help',
  'help-tab-shortcuts': 'Shortcuts',
  'help-tab-search': 'Search',
  'help-filter-shortcuts': '…filter shortcuts',
  'help-filter-search': '…filter search filters',
  'help-together': 'Putting it together',
  'help-together-note': 'From Sam, since June, with a file attached, mentioning the annex. The filter buttons type this for you.',
  'help-foot': 'Single-key shortcuts pause while you are typing in a text field. Anything Petrel does not recognise as a filter is searched as ordinary text.',
  'close': 'Close',
  'close-title': 'Close (Esc)',
  'palette-placeholder': 'Type a command…',
  'palette-group-conversation': 'This conversation',
  'palette-group-goto': 'Go to',
  'palette-group-app': 'Petrel',
  'palette-empty': 'Nothing matches “{ $query }”',
  'palette-navigate': 'navigate',
  'palette-run': 'run',
  'palette-all-shortcuts': 'all shortcuts',
  'palette-more': '{ $count } more results',
  'not-implemented': '{ $label } is not built yet',
  'account-switched': 'Switched to { $email }',
  'account-none-at': 'No account at { $n }',

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
