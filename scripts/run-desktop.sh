#!/usr/bin/env bash
# Build, bundle, and launch Petrel so it actually stays running.
#
# Launched from a non-interactive shell, the process lands in a background
# process group and gets SIGTTOU/SIGTTIN'd into a stopped (T) state the moment
# it touches the terminal. A stopped process keeps its window on screen showing
# the last frame it drew — so the app looks perfect and responds to nothing.
# This launches it detached, then makes sure it is actually running.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

(cd apps/desktop/ui && npm run build >/dev/null 2>&1)
cargo build --release -p petrel-desktop --features custom-protocol 2>&1 | tail -1
"$ROOT/scripts/make-app-bundle.sh" >/dev/null

pkill -9 -f petrel-desktop 2>/dev/null || true
rm -f "$HOME/Library/Application Support/Petrel/frontend.log"
sleep 1

open "$ROOT/target/Petrel.app"

# Resume it if the launch context suspended it, and keep checking briefly —
# the stop can land a moment after start.
for _ in $(seq 1 10); do
  sleep 1
  PID="$(pgrep -f 'Petrel.app.*petrel-desktop' | head -1 || true)"
  [ -n "$PID" ] || continue
  STATE="$(ps -o stat= -p "$PID" | tr -d ' ')"
  case "$STATE" in T*) kill -CONT "$PID" 2>/dev/null || true ;; esac
done

PID="$(pgrep -f 'Petrel.app.*petrel-desktop' | head -1 || true)"
if [ -z "$PID" ]; then echo "FAILED: not running"; exit 1; fi
echo "pid $PID state $(ps -o stat= -p "$PID" | tr -d ' ')"
