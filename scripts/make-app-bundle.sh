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
# The one place a version is declared is tauri.conf.json — it is what the
# running app reports as its own version, and therefore what the updater
# compares against a release. A plist that hardcodes a different number
# makes the bundle disagree with the program inside it: Finder says one
# thing, the Updates pane another, and a local update test compares
# versions that were never the same number.
CONF="$ROOT/apps/desktop/src-tauri/tauri.conf.json"
VERSION="$(sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$CONF" | head -1)"
[ -n "$VERSION" ] || { echo "no version in $CONF" >&2; exit 1; }

[ -x "$BIN" ] || { echo "no binary at $BIN — build it first" >&2; exit 1; }

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$ROOT/assets/Petrel.icns" "$APP/Contents/Resources/Petrel.icns"
cp "$BIN" "$APP/Contents/MacOS/petrel-desktop"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key>              <string>Petrel</string>
  <key>CFBundleDisplayName</key>       <string>Petrel</string>
  <key>CFBundleExecutable</key>        <string>petrel-desktop</string>
  <key>CFBundleIconFile</key>          <string>Petrel</string>
  <key>CFBundleIdentifier</key>        <string>dev.petrel.desktop</string>
  <key>CFBundleVersion</key>           <string>$VERSION</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundlePackageType</key>       <string>APPL</string>
  <key>LSMinimumSystemVersion</key>    <string>11.0</string>
  <key>NSHighResolutionCapable</key>   <true/>
</dict>
</plist>
PLIST

# Signing decides whether the keychain trusts the app across rebuilds.
# Keychain ACLs bind to the code signature, through the designated
# requirement macOS derives from it. Get that wrong and every rebuild looks
# like a different app, so the account passwords are asked for again.
#
# Developer ID first, when there is one. Its designated requirement is built
# from the team id and the bundle id, both of which survive a rebuild, so one
# "Always Allow" holds for good. It also makes this bundle the same identity
# as the released app, so consent granted to one covers the other instead of
# each asking separately.
#
# "Petrel Dev" — a self-signed code-signing certificate (Keychain Access →
# Certificate Assistant → Create a Certificate → type: Code Signing) — is the
# fallback for anyone without an Apple account. Stable across rebuilds, but it
# carries no team identifier, so its requirement is weaker and it is a
# different identity from the released app.
#
# Ad-hoc is the last resort: unique per build, so macOS re-asks every time.
IDENTITY=""
LABEL=""
if [ -n "${PETREL_SIGN_IDENTITY:-}" ]; then
  IDENTITY="$PETREL_SIGN_IDENTITY"
  LABEL="$PETREL_SIGN_IDENTITY"
else
  # `|| true`: grep finds nothing when there is no such certificate, and under
  # `set -e` a failing pipeline would end the script before the fallbacks.
  DEVID="$(security find-identity -v -p codesigning 2>/dev/null \
    | grep 'Developer ID Application' | head -1 \
    | sed -E 's/.*"(.*)".*/\1/' || true)"
  if [ -n "$DEVID" ]; then
    IDENTITY="$DEVID"
    LABEL="$DEVID (same identity as the released app)"
  elif security find-identity -v -p codesigning 2>/dev/null | grep -q "Petrel Dev"; then
    IDENTITY="Petrel Dev"
    LABEL="Petrel Dev"
  fi
fi

if [ -n "$IDENTITY" ]; then
  if codesign --force --deep --sign "$IDENTITY" "$APP" >/dev/null 2>&1; then
    echo "signed as $LABEL — keychain consent will persist"
  else
    # Loud, because this fallback once hid for a day: an ad-hoc bundle is a
    # brand-new app to macOS and every keychain consent starts over.
    echo "WARNING: signing as '$IDENTITY' FAILED — falling back to ad-hoc; keychain will re-ask" >&2
    codesign --force --deep --sign - "$APP" >/dev/null 2>&1 || true
  fi
else
  echo "note: no signing identity found; ad-hoc signature (keychain re-asks every rebuild)" >&2
  codesign --force --deep --sign - "$APP" >/dev/null 2>&1 || true
fi
echo "$APP ($VERSION)"
