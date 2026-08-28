# Security

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

macOS builds are also signed with an Apple Developer ID and notarized.

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

