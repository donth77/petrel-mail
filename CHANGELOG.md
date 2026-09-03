# Changelog

## 1.0.0

The first release.

Petrel is a desktop email client for macOS. It talks to your mail server over IMAP and
SMTP. Your mail is stored on your own machine, in SQLite and ordinary files. Nothing
goes through a service in the middle.

### Mail

- IMAP and SMTP. As many accounts as you want.
- Type your email address and Petrel works out the server settings. It knows 18
  providers by name, and falls back to looking them up.
- Gmail and iCloud work with app passwords.
- Messages are grouped into conversations. On Gmail it uses Gmail's own conversation
  ids, so the grouping matches what you see in the web app.
- New mail appears as it arrives. Petrel does not wait for a timer.

### Reading

- Message HTML is cleaned up and then shown in a sandbox. No scripts run. It has no
  network access.
- Remote images and tracking pixels are blocked until you allow them. You can allow
  one message or one sender.
- Attachments preview, save and open. You get a warning before opening anything that
  can run.
- Images sent inside a message show inline.
- In dark mode, mail that only ships a light design gets darkened. Photos are left
  alone. Any message can be switched back to light.
- Meeting invitations show as a card with Accept, Tentative and Decline on it. Your
  answer is emailed back to the organiser.
- Links open in your browser. Petrel warns you first if a web address uses characters
  that make it look like a different site.

### Writing

- Rich text editor. Sends HTML with a plain text copy alongside.
- Reply, reply to all, and forward, with the quoting people expect.
- A signature for each account. Changing who the mail is from changes the signature.
- Drafts save as you type, on your machine and on the server.
- Undo send. You choose how long the pause is, up to 30 seconds.
- If you write "attached" and there is no attachment, Petrel says so before sending.
- Addresses complete as you type, based on who you write to most and most recently.

### Organising

- Search your mail instantly. It understands terms like
  `from:alice has:attachment before:2026-01-01`.
- Archive, move, star, delete, mark as spam. Every one of them can be undone for ten
  seconds.
- Tags. On Gmail they become labels. On other servers they become IMAP keywords.
  Either way they sync both directions.
- Rules that file mail as it arrives.
- The Trash can empty itself after 7, 30 or 90 days. This is off unless you turn it
  on. The clock starts when a message reaches the Trash, not when it was sent, so
  binning an old message does not delete it straight away.

### The rest

- Keyboard shortcuts for everything, with a list you can search. Press ⌘K to run any
  command by name.
- Notifications, with a pause button for 30 minutes, an hour, or until tomorrow. It
  silences Petrel only.
- Import mbox files and `.eml` files. Export any folder to mbox.
- Updates are signed, and only install when you ask. Petrel never checks on its own.
- Works with a keyboard alone. Every control is labelled for a screen reader.
- Eight languages: English, German, Spanish, French, Japanese, Korean, Brazilian
  Portuguese and Simplified Chinese. Petrel follows your system language, and you
  can pick another in Settings.

### What is missing

Worth knowing before you install:

- macOS only.
- Outlook.com and Microsoft 365 mailboxes need Microsoft's own sign-in, which Petrel
  does not have yet. They cannot be added in this version.
- Gmail and Microsoft 365 are reached over IMAP, not their own APIs. This works, but
  Gmail prefers its API and may tighten IMAP access later.
- Notifications have no buttons on them. The macOS notification API Petrel uses does
  not offer any.

### Requirements

macOS 11 or later, on Apple silicon or Intel.
