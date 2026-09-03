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
cp "$root/scripts/harness/scroll-probe.js" "$out/scroll-probe.js"
cp "$root/scripts/harness/fit-probe.js" "$out/fit-probe.js"
cp "$root/scripts/harness/thread-probe.js" "$out/thread-probe.js"
cp "$root/apps/desktop/src-tauri/src/height_reporter.js" "$out/height_reporter.js"

# Inject the shim ahead of the module script. A classic script tag runs during
# parsing, so window.__TAURI_INTERNALS__ exists before the deferred module ever
# evaluates — which is the only ordering that works.
python3 - "$out/index.html" "$out/shim.js" "$out/scroll-probe.js" "$out/fit-probe.js" "$out/thread-probe.js" <<'PY'
import hashlib, re, sys
path, shim, scroll, fit, thread = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4], sys.argv[5]
html = open(path).read()
if 'shim.js' not in html:
    # Stamped with the shim's own hash. Without it the browser keeps the copy
    # it fetched the first time and every later change to the shim is invisible
    # — which is worse than no shim, because the harness then reports on code
    # nobody is running. Cost an afternoon once; the URL is cheap.
    tag = hashlib.blake2s(open(shim, 'rb').read(), digest_size=6).hexdigest()
    html = re.sub(r'(<script type="module")',
                  f'<script src="./shim.js?v={tag}"></script>\n    \\1', html, count=1)
    print(f"shim injected (v={tag})")
def inject(name, src):
    global html
    if name in html:
        return
    tag = hashlib.blake2s(open(src, 'rb').read(), digest_size=6).hexdigest()
    html = re.sub(r'(<script type="module")',
                  f'<script src="./{name}?v={tag}"></script>\n    \\1', html, count=1)
    print(f"{name} injected (v={tag})")
inject('scroll-probe.js', scroll)
inject('fit-probe.js', fit)
inject('thread-probe.js', thread)
open(path, 'w').write(html)
PY

echo "serving $out on http://localhost:$port"
cd "$out"
exec python3 -m http.server "$port"
