#!/usr/bin/env bash
# One command from source change to a relaunchable app bundle.
#
# There is no hot reload here: this environment suspends any process it spawns,
# so the vite dev server cannot stay up and the app must embed its assets. UI
# change -> run this -> quit and reopen Petrel.app.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
# Point git at the tracked hooks. core.hooksPath is local config, so a fresh
# clone has no hooks until something sets it; doing it here means the first
# build arms the pre-commit fmt check. Idempotent, and quiet when already set.
[ "$(git config --get core.hooksPath || true)" = ".githooks" ] || \
    git config core.hooksPath .githooks

# Formatting is part of the build. This rewrites the tree rather than checking
# it, which is convenient but was hiding unformatted commits: the reformat
# landed in whatever commit came next, so anything committed before a rebuild
# reached CI unformatted. .githooks/pre-commit is what actually catches that;
# this line just means you rarely see it fire.
cargo fmt --all
(cd apps/desktop/ui && pnpm run build >/dev/null)
cargo build --release -p petrel-desktop --features custom-protocol 2>&1 | tail -1
"$ROOT/scripts/make-app-bundle.sh"
