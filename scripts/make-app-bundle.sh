#!/usr/bin/env bash
# Wraps the built binary in a minimal .app bundle.
#
# macOS treats a bare executable and a bundled app differently: an unbundled
# binary launched outside LaunchServices never becomes a proper foreground app,
# so its window renders and can be dragged but its webview never receives focus
# or input. Running `open` on a bundle goes through LaunchServices and fixes it.
#
# This is a dev convenience; release bundling goes through `tauri build`.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="${1:-$ROOT/target/release/petrel-desktop}"
APP="$ROOT/target/Petrel.app"

[ -x "$BIN" ] || { echo "no binary at $BIN — build it first" >&2; exit 1; }

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$ROOT/assets/Petrel.icns" "$APP/Contents/Resources/Petrel.icns"
cp "$BIN" "$APP/Contents/MacOS/petrel-desktop"

cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key>              <string>Petrel</string>
  <key>CFBundleDisplayName</key>       <string>Petrel</string>
  <key>CFBundleExecutable</key>        <string>petrel-desktop</string>
  <key>CFBundleIconFile</key>          <string>Petrel</string>
  <key>CFBundleIdentifier</key>        <string>dev.petrel.desktop</string>
  <key>CFBundleVersion</key>           <string>0.0.1</string>
  <key>CFBundleShortVersionString</key><string>0.0.1</string>
  <key>CFBundlePackageType</key>       <string>APPL</string>
  <key>LSMinimumSystemVersion</key>    <string>10.15</string>
  <key>NSHighResolutionCapable</key>   <true/>
</dict>
</plist>
PLIST

# Ad-hoc signature: unsigned bundles are quarantined and refused on launch.
codesign --force --deep --sign - "$APP" >/dev/null 2>&1 || true
echo "$APP"
