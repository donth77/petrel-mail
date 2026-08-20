# Dovecot test server — NOT CURRENTLY WORKING

Superseded by GreenMail (see `../README.md`). Kept for whoever wants to finish it: the 2.4
config below parses (`doveconf -n` is clean) but the container dies with
`default_login_user doesn't exist: dovenull` — the official image ships no dovecot system
users, so a replacement `dovecot.conf` must define users that actually exist in it.

## Original notes


Throwaway IMAP server for integration tests. Plaintext, loopback-only, synthetic mail only.

    docker run -d --rm --name petrel-dovecot -p 3143:143 \
      -v "$PWD/testkit/dovecot/dovecot.conf:/etc/dovecot/dovecot.conf:ro" \
      dovecot/dovecot:2.3-latest

Account: `petrel@example.com` / `petrelpass` (any username works — static passdb).
Stop with `docker stop petrel-dovecot`.
