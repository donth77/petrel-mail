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
- Your version of Petrel, and your operating system.
- A fix, if you have one in mind. Not required.

You will get a reply within 72 hours saying whether we can reproduce it. If a
fix is needed, you will hear what it is and when it ships. If we think the
report is not a vulnerability, you will hear why, not silence.

## Security model

What stands between a stranger's message and your mail, and where the
interesting edges are if you go looking.

| | |
| --- | --- |
| **Your mail** | Moves between your computer and your provider and stops there. No server of ours in the middle, and no telemetry. |
| **Your passwords** | Held in the operating system's credential store: Keychain, Credential Manager or Secret Service. Never in a file, the database, or a log. |
| **Message HTML** | Three layers, each catching what the others miss. An allowlist sanitizer strips anything not permitted and falls back to plain text when it cannot parse. The body then renders in a sandboxed frame on its own `petrel-msg://` origin. A per-message CSP blocks network egress from inside it. |
| **Remote images** | Blocked until you ask, and Petrel says how many it blocked. The difference between reading a message and telling the sender you read it. |
| **Deceptive links** | A link whose text and destination disagree asks first. An address can be spelled with letters that look identical to Latin ones. |
| **Attachments** | One that is a program rather than a document says so before it opens. |
| **Updates** | Refused unless the signature matches, before any of it runs. |
| **Connections** | TLS with certificate validation, on both the incoming and outgoing server. |

So: scripts in a message never run, its forms cannot submit, its links open in
your browser, and nothing it loads carries a referrer.

macOS builds are also signed with an Apple Developer ID and notarized. Windows
and Linux packages are not code-signed yet, so your system may warn you when
you install one.

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

- Anything that needs an attacker to already have your unlocked computer.
- Bugs in your mail provider, or in your operating system.
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
