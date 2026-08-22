#!/usr/bin/env bash
# Launch Petrel against a real mailbox, reading credentials from .env.local.
#
# Why this exists rather than "export and run": the app has to be launched
# through LaunchServices to get real keyboard focus on macOS, and LaunchServices
# does not inherit the calling shell's environment — so exporting PETREL_IMAP_*
# and running `open` silently launches an app with no account, which looks
# exactly like a broken sync. `open --env` passes them explicitly.
#
# The values are never written into the bundle, never logged, and never leave
# this process tree. Use an app-specific password (see .env.example) and revoke
# it when you are done.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

ENV_FILE="${PETREL_ENV_FILE:-$ROOT/.env.local}"
if [ ! -f "$ENV_FILE" ]; then
  echo "No $ENV_FILE. Start from the template:" >&2
  echo "  cp .env.example .env.local && \$EDITOR .env.local" >&2
  exit 1
fi

set -a
# shellcheck disable=SC1090
. "$ENV_FILE"
set +a

: "${PETREL_IMAP_HOST:?set PETREL_IMAP_HOST in $ENV_FILE}"
: "${PETREL_IMAP_USER:?set PETREL_IMAP_USER in $ENV_FILE}"
: "${PETREL_IMAP_PASS:?set PETREL_IMAP_PASS in $ENV_FILE}"

# A separate store by default, so a real mailbox never lands on top of the demo
# data you have been triaging — and so wiping one does not cost you the other.
DATA_DIR="${PETREL_DATA_DIR:-$HOME/Library/Application Support/Petrel-live}"
mkdir -p "$DATA_DIR"

echo "account : $PETREL_IMAP_USER @ $PETREL_IMAP_HOST:${PETREL_IMAP_PORT:-993}"
echo "store   : $DATA_DIR"

pkill -9 -f petrel-desktop 2>/dev/null || true
sleep 1

open -n "$ROOT/target/Petrel.app" \
  --env "PETREL_IMAP_HOST=$PETREL_IMAP_HOST" \
  --env "PETREL_IMAP_PORT=${PETREL_IMAP_PORT:-993}" \
  --env "PETREL_IMAP_USER=$PETREL_IMAP_USER" \
  --env "PETREL_IMAP_PASS=$PETREL_IMAP_PASS" \
  --env "PETREL_DATA_DIR=$DATA_DIR"

echo
echo "Watching the sync (ctrl-C to stop tailing; the app keeps running):"
sleep 2
tail -f "$DATA_DIR/frontend.log" 2>/dev/null || echo "no log yet at $DATA_DIR/frontend.log"
