#!/usr/bin/env bash
# Serve the *production* UI bundle with a stand-in for Tauri's IPC layer.
#
# The dev server is not a substitute. It serves a different build against a
# different api module, so anything that only breaks in the shipped bundle is
# invisible there. This runs the exact bytes the desktop app embeds, in a
# browser that can be driven and inspected.
#
#   ./scripts/harness.sh          build the UI, then serve on :5199
#   ./scripts/harness.sh --no-build  serve whatever is already in dist/
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ui="$root/apps/desktop/ui"
out="$root/.harness"
port="${PETREL_HARNESS_PORT:-5199}"

if [ "${1:-}" != "--no-build" ]; then
  pnpm --dir "$ui" build
fi

rm -rf "$out"
mkdir -p "$out"
cp -R "$ui/dist/." "$out/"
cp "$root/scripts/harness/shim.js" "$out/shim.js"
cp "$root/scripts/harness/msg.html" "$out/msg.html"

# Inject the shim ahead of the module script. A classic script tag runs during
# parsing, so window.__TAURI_INTERNALS__ exists before the deferred module ever
# evaluates — which is the only ordering that works.
python3 - "$out/index.html" <<'PY'
import re, sys
path = sys.argv[1]
html = open(path).read()
if 'shim.js' not in html:
    html = re.sub(r'(<script type="module")', '<script src="./shim.js"></script>\n    \\1', html, count=1)
    open(path, 'w').write(html)
    print("shim injected")
PY

echo "serving $out on http://localhost:$port"
cd "$out"
exec python3 -m http.server "$port"
