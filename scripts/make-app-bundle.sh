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

# Signing decides whether the keychain trusts the app across rebuilds.
# Keychain ACLs bind to the code signature; an ad-hoc signature is unique
# per build, so every rebuild is "a different app" and macOS re-asks for the
# account passwords. A stable local identity — a self-signed code-signing
# certificate named "Petrel Dev" (Keychain Access → Certificate Assistant →
# Create a Certificate → type: Code Signing) — keeps the identity constant,
# and one "Always Allow" then holds through every rebuild. Used when present;
# ad-hoc otherwise, which at least clears quarantine.
if security find-identity -v -p codesigning 2>/dev/null | grep -q "Petrel Dev"; then
  codesign --force --deep --sign "Petrel Dev" "$APP" >/dev/null 2>&1     && echo "signed as Petrel Dev (keychain consent will persist)"     || codesign --force --deep --sign - "$APP" >/dev/null 2>&1 || true
else
  codesign --force --deep --sign - "$APP" >/dev/null 2>&1 || true
fi
echo "$APP"
