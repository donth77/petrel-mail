# Test servers

Throwaway mail servers for integration tests. **Synthetic mail only** — never point these
at real accounts, and never commit real addresses or recorded traffic (see AGENTS.md).

## GreenMail (default — fast, zero config, native arm64)

    docker run -d --rm --name petrel-greenmail -p 3143:3143 -p 3025:3025 \
      -e GREENMAIL_OPTS='-Dgreenmail.setup.test.all -Dgreenmail.hostname=0.0.0.0 \
         -Dgreenmail.users=petrel:petrelpass -Dgreenmail.verbose' \
      greenmail/standalone:latest

IMAP on 3143, SMTP on 3025. Stop: `docker stop petrel-greenmail`.

**Quirk that will bite you:** with `-Dgreenmail.users=petrel:petrelpass@example.com`, the
account's *address* is `petrel@example.com` but its *IMAP login* is the local part,
`petrel`. Logging in with the full address fails with "Invalid login/password". Mail must be
addressed to `petrel@example.com` to be delivered, and read by logging in as `petrel`.

Its capabilities are deliberately modest — `IMAP4REV1 IDLE UIDPLUS MOVE SORT QUOTA LITERAL+
SASL-IR AUTH=XOAUTH2`, with **no CONDSTORE/QRESYNC** — which makes it the reference test for
the sync ladder's bottom rung (full reconcile), the same rung Microsoft 365 IMAP forces on us.

Run the IMAP slice against it:

    cargo test -p petrel-providers --features insecure-plaintext \
      --test imap_slice -- --ignored --nocapture

The test appends messages and does not clean up; restart the container for a fresh mailbox
(it runs with `--rm`, so state is ephemeral).

## Live provider testing (real account)

Credentials come from the environment — never from a command line, a committed file, or a
chat transcript:

    cp .env.example .env.local      # .env.local is gitignored
    $EDITOR .env.local              # fill in host/user/app-password
    set -a && . ./.env.local && set +a
    cargo test -p petrel-providers --test imap_slice live_provider -- --ignored --nocapture

Rules for this path:

* **Throwaway account only.** The fault-injection tests interrupt sends and kill the process
  mid-operation; never aim them at mail you care about.
* **App-specific password, not the account password** — scoped, revocable, and the only
  thing Gmail/iCloud accept for IMAP. Revoke it when finished.
* **`live_provider_probe` is read-only** (no APPEND, no flag writes) and **redacts output**:
  it prints capabilities, counts, UIDs, and sizes, but subjects and addresses appear only as
  `«N chars»`. Real mail content must never reach a terminal transcript or an agent's context.

## Wanted: a CONDSTORE/QRESYNC server (M1 task)

The upper rungs of the ladder need Stalwart or Cyrus. Dovecot was tried and abandoned here:
its official arm64 image is 2.4 with a rewritten config format and no `dovenull` system user,
and the 2.3 amd64 image crashes under Rosetta on Apple Silicon. `dovecot/` keeps the
half-finished 2.4 config for whoever picks this up next.
