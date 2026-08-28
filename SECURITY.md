# Security

Petrel handles your mail and your passwords. If you find a way to get at either
one, please tell us before you tell anyone else.

## Supported versions

| Version | Security fixes |
| ------- | -------------- |
| Latest release | Yes |
| Anything older | No |

Fixes go into the next release. There are no backports, so the answer to "am I
patched?" is "am I on the latest version?".

## Reporting a vulnerability

Use [GitHub's private vulnerability
reporting](https://github.com/donth77/petrel-mail/security/advisories/new). It
opens a report only you and the maintainers can read.

Please do not open a public issue for a security bug. A public issue is a
working exploit handed to everyone who reads it, including people running the
version you just broke.

It helps to include:

- What the bug lets an attacker do.
- The steps to reproduce it, and the message or file that triggers it if there
  is one. A `.eml` is ideal.
- Your version of Petrel and of macOS.
- A fix, if you have one in mind. Not required.

You will get a reply within 72 hours saying whether we can reproduce it. If a
fix is needed, you will hear what it is and when it ships. If we think the
report is not a vulnerability, you will hear why, not silence.

## How Petrel is built to fail safely

Some of this is worth knowing before you go looking, because it says where the
interesting edges are.

**There is no server.** Mail moves between your Mac and your provider and
stops there. No account of ours sits in the middle, because there is no account
of ours. Nothing is uploaded, and there is no telemetry to turn off.

**Passwords live in the macOS keychain**, never in a file, never in the
database, never in a log. The database holds your mail; taking a copy of it
gets someone your mail, not your account.

**A message is treated as hostile HTML written by a stranger**, because that is
what it is. Three separate layers stand between it and you, and each one blocks
something the others do not:

- The sanitizer works from an allowlist. Anything not explicitly permitted is
  removed, CSS declarations are filtered, and `url()` is banned outright inside
  styles. Anything it cannot parse degrades to plain text rather than being
  passed along.
- Message bodies render in a sandboxed frame under their own `petrel-msg://`
  origin, so a message cannot reach the app's own page or its data.
- A per-message CSP blocks network egress from inside that frame.

The practical results: scripts in messages never run, forms in messages cannot
submit anywhere, links open in your browser rather than inside Petrel, and
nothing a message loads carries a referrer.

**Remote images are blocked until you ask for them**, and the count of what was
blocked is shown to you. That is the difference between reading a message and
telling the sender you read it.

**Links that read as one address and go to another ask first.** An address can
be spelled with letters from other alphabets that look identical to Latin ones,
and Petrel says so rather than trusting the rendering.

**Attachments that are programs say so before they open.** Most malware arrives
by email, and the confirmation names the file and what running it means.

**Updates are signed.** An update whose signature does not match is refused
before any of it runs. Releases are also signed with an Apple Developer ID and
notarized.

**Connections to your provider are TLS with certificate validation.** Petrel
tests both the incoming and outgoing server when you add an account, and says
so plainly if either one cannot be reached safely.

## In scope

- Running code, reading files, or reaching the network from a message.
- Escaping the message sandbox, or reaching app state or another message's
  content from inside it.
- Getting a password out of the keychain, out of the database, or into a log.
- Making Petrel talk to a server it should not, or accept a certificate it
  should not.
- Getting an unsigned or wrongly signed update installed.
- Making a link, an address, or an attachment appear to be something it is not.
- Reading or modifying another account's mail from within one account.

## Out of scope

- Anything that needs an attacker to already have your unlocked Mac.
- Bugs in your mail provider, or in macOS itself.
- Denial of service by feeding Petrel an enormous or malformed mailbox, unless
  it corrupts stored mail or leaks something.
- Missing hardening that has no demonstrated impact. Tell us anyway if you
  think it matters, but say what it lets an attacker do.
- Reports from an automated scanner with no working reproduction.

## Disclosure

We aim to ship a fix within 90 days of a confirmed report, and would rather
disclose together than have you wait indefinitely on us. If a fix is going to
take longer than that, you will be told why and can publish on your own
schedule.

You will be credited by whatever name you give us, or not at all if you prefer.
