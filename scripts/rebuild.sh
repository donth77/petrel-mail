#!/usr/bin/env bash
# One command from source change to a relaunchable app bundle.
#
# There is no hot reload here: this environment suspends any process it spawns,
# so the vite dev server cannot stay up and the app must embed its assets. UI
# change -> run this -> quit and reopen Petrel.app.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
(cd apps/desktop/ui && npm run build >/dev/null)
cargo build --release -p petrel-desktop --features custom-protocol 2>&1 | tail -1
"$ROOT/scripts/make-app-bundle.sh"
