#!/usr/bin/env bash
# Launch Petrel on a throwaway mailbox of synthetic mail — for screenshots,
# for showing the app to someone, for looking at the UI without an account.
#
# Nothing here touches your real store: the data directory is passed
# explicitly, and it is passed with `open --env` because LaunchServices does
# not inherit the calling shell's environment (exporting it and running `open`
# launches the app pointed at your actual mail, which is the one outcome this
# script exists to prevent).
#
#   ./scripts/run-demo.sh              a fresh mailbox in a temp directory
#   ./scripts/run-demo.sh ~/demo-store keep it somewhere, reuse it next time
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

DATA_DIR="${1:-$(mktemp -d -t petrel-demo)}"
mkdir -p "$DATA_DIR"

[ -d "$ROOT/target/Petrel.app" ] || { echo "no bundle — run ./scripts/rebuild.sh first" >&2; exit 1; }

# Only this script's own instance is stopped. A Petrel running on real mail is
# left alone; the two coexist, on separate stores.
if PID="$(lsof -t "$DATA_DIR/petrel.db" 2>/dev/null | head -1)" && [ -n "$PID" ]; then
  kill "$PID" 2>/dev/null || true
  sleep 2
fi

echo "store : $DATA_DIR"
open -n "$ROOT/target/Petrel.app" --env "PETREL_DATA_DIR=$DATA_DIR"

# Seeding is 10,000 synthetic messages and the filing that follows it. Waiting
# for the Inbox to actually hold mail is the difference between a screenshot of
# the app and a screenshot of an empty window.
echo -n "seeding"
for _ in $(seq 1 60); do
  sleep 1; echo -n "."
  n="$(sqlite3 "file:$DATA_DIR/petrel.db?mode=ro" \
    "SELECT count(*) FROM placements p JOIN folders f ON f.id = p.folder_id WHERE f.role='inbox';" 2>/dev/null || echo 0)"
  if [ "${n:-0}" -gt 0 ]; then
    echo " ready — $n messages in the inbox"
    echo
    echo "Screenshot: ⌘⇧4, then Space, then click the window."
    exit 0
  fi
done
echo " still seeding; give it a moment"
