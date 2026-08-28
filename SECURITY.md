# Security

Petrel handles your mail and your passwords. If you find a way to get at either
one, please tell us before you tell anyone else.


## How Petrel is built to fail safely

Worth knowing before you go looking, because it says where the interesting
edges are.

**There is no server.** Mail moves between your computer and your provider and
stops there. Nothing is uploaded, and there is no telemetry.

**Passwords live in the operating system's credential store**, never in a file,
the database, or a log. That is Keychain on macOS, Credential Manager on
Windows, and Secret Service on Linux. The database holds your mail, so a copy
of it gets someone your mail, not your account.

**A message is hostile HTML written by a stranger.** Three layers sit between
it and you, and each blocks something the others do not:

1. The sanitizer works from an allowlist. Anything not permitted is removed,
   CSS declarations are filtered, `url()` is banned inside styles, and anything
   it cannot parse degrades to plain text.
2. The body renders in a sandboxed frame under its own `petrel-msg://` origin,
   so it cannot reach the app's page or its data.
3. A per-message CSP blocks network egress from inside that frame.

The result: scripts never run, forms cannot submit, links open in your browser,
and nothing a message loads carries a referrer.

**Remote images are blocked until you ask for them**, and Petrel tells you how
many it blocked. That is the difference between reading a message and telling
the sender you read it.

**Two things ask before they can hurt you:** a link whose text and destination
disagree, because an address can be spelled with letters that look identical to
Latin ones, and an attachment that is a program rather than a document.

**Updates are checked against a signing key.** One whose signature does not
match is refused before any of it runs.

**Mail connections use TLS with certificate validation**, tested against both
the incoming and outgoing server when you add an account.

One platform difference worth stating: macOS builds are also signed with an
Apple Developer ID and notarized. Windows and Linux packages are not
code-signed yet, so your system may warn you when you install one.

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
