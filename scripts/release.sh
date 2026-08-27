#!/usr/bin/env bash
# One command from a clean tree to a notarized, stapled Petrel.dmg.
#
# What this needs from you, once:
#
#   * An Apple Developer account, and a **Developer ID Application**
#     certificate installed in this login keychain. Not "Apple Development"
#     and not the self-signed "Petrel Dev" the daily builds use: only
#     Developer ID is accepted for software distributed outside the App
#     Store, and notarization refuses anything else.
#   * A notarytool credential. Either a stored profile (recommended):
#         xcrun notarytool store-credentials petrel-notary \
#           --apple-id you@example.com --team-id ABCDE12345 \
#           --password <app-specific-password>
#     or the three variables APPLE_ID, APPLE_TEAM_ID and APPLE_APP_PASSWORD
#     in the environment. The password is an app-specific one from
#     appleid.apple.com — never your Apple ID password.
#
# Everything else is checked before any work starts, because finding out
# after a five-minute build that a certificate is missing is the kind of
# thing that makes people stop cutting releases.
#
#   ./scripts/release.sh 0.1.0
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

VERSION="${1:-}"
if [ -z "$VERSION" ]; then
  echo "usage: $0 <version>   e.g. $0 0.1.0" >&2
  exit 2
fi

APP="$ROOT/target/release/Petrel.app"
DMG="$ROOT/target/release/Petrel-$VERSION.dmg"
ENTITLEMENTS="$ROOT/apps/desktop/src-tauri/Entitlements.plist"
NOTARY_PROFILE="${PETREL_NOTARY_PROFILE:-petrel-notary}"

say() { printf '\n== %s\n' "$1"; }
die() { printf 'release: %s\n' "$1" >&2; exit 1; }

# ---------------------------------------------------------------- preflight
say "preflight"

IDENTITY="${APPLE_SIGNING_IDENTITY:-}"
if [ -z "$IDENTITY" ]; then
  # `|| true`: with no matching certificate the pipeline exits non-zero, and
  # under `set -e` that ends the script *before* the explanation below —
  # a release tool that fails without saying why is the thing this script
  # exists to prevent.
  IDENTITY="$(security find-identity -v -p codesigning 2>/dev/null \
    | grep 'Developer ID Application' | head -1 \
    | sed -E 's/.*"(.*)".*/\1/' || true)"
fi
[ -n "$IDENTITY" ] || die "no Developer ID Application certificate in the keychain.
  Install one from developer.apple.com (Certificates → + → Developer ID
  Application), or set APPLE_SIGNING_IDENTITY to its exact name."
echo "identity: $IDENTITY"

if xcrun notarytool history --keychain-profile "$NOTARY_PROFILE" >/dev/null 2>&1; then
  NOTARY_ARGS=(--keychain-profile "$NOTARY_PROFILE")
  echo "notary: keychain profile '$NOTARY_PROFILE'"
elif [ -n "${APPLE_ID:-}" ] && [ -n "${APPLE_TEAM_ID:-}" ] && [ -n "${APPLE_APP_PASSWORD:-}" ]; then
  NOTARY_ARGS=(--apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" --password "$APPLE_APP_PASSWORD")
  echo "notary: APPLE_ID / APPLE_TEAM_ID / APPLE_APP_PASSWORD"
else
  die "no notarization credential. Store one:
  xcrun notarytool store-credentials $NOTARY_PROFILE \\
    --apple-id you@example.com --team-id ABCDE12345 --password <app-specific>
or export APPLE_ID, APPLE_TEAM_ID and APPLE_APP_PASSWORD."
fi

[ -f "$ENTITLEMENTS" ] || die "missing $ENTITLEMENTS"
command -v hdiutil >/dev/null || die "hdiutil not found (is this macOS?)"

# A release built from a dirty tree is a release nobody can reproduce.
if [ -n "$(git status --porcelain)" ]; then
  echo "warning: working tree is dirty; this build will not be reproducible" >&2
fi
echo "commit: $(git rev-parse --short HEAD)"

# ------------------------------------------------------------------- build
say "build"
(cd apps/desktop/ui && pnpm run build >/dev/null)
# custom-protocol is what puts Tauri in production mode; without it the
# webview goes looking for a dev server that is not there.
cargo build --release -p petrel-desktop --features custom-protocol

say "assemble"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$ROOT/assets/Petrel.icns" "$APP/Contents/Resources/Petrel.icns"
cp "$ROOT/target/release/petrel-desktop" "$APP/Contents/MacOS/petrel-desktop"
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
  <key>LSMinimumSystemVersion</key>    <string>10.15</string>
  <key>NSHighResolutionCapable</key>   <true/>
  <key>LSApplicationCategoryType</key> <string>public.app-category.productivity</string>
  <key>NSHumanReadableCopyright</key>  <string>Petrel</string>
</dict>
</plist>
PLIST

# --------------------------------------------------------------------- sign
say "sign"
# --options runtime is the hardened runtime, which notarization requires;
# --timestamp asks Apple's timestamp server so the signature outlives the
# certificate. Both are refusal reasons if omitted, discovered after upload.
codesign --force --deep --options runtime --timestamp \
  --entitlements "$ENTITLEMENTS" --sign "$IDENTITY" "$APP"
codesign --verify --deep --strict --verbose=2 "$APP"

# ---------------------------------------------------------------------- dmg
say "package"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
rm -f "$DMG"
hdiutil create -volname "Petrel $VERSION" -srcfolder "$STAGE" \
  -ov -format ULFO "$DMG" >/dev/null
codesign --force --timestamp --sign "$IDENTITY" "$DMG"

# --------------------------------------------------------------- notarize
say "notarize (this waits on Apple; a few minutes is normal)"
xcrun notarytool submit "$DMG" "${NOTARY_ARGS[@]}" --wait

say "staple"
# Stapling puts the ticket inside the file, so a machine that has never
# spoken to Apple — or is offline when the DMG is opened — still sees a
# notarized app rather than a warning.
xcrun stapler staple "$DMG"
xcrun stapler validate "$DMG"

say "verify as a fresh Mac would"
# The real test: what Gatekeeper says about the app inside, not what we
# believe about the file we just made.
VERIFY="$(mktemp -d)"
hdiutil attach -nobrowse -quiet -mountpoint "$VERIFY/mnt" "$DMG"
spctl --assess --type execute --verbose=4 "$VERIFY/mnt/Petrel.app"
hdiutil detach -quiet "$VERIFY/mnt"
rm -rf "$VERIFY"

printf '\n%s\n' "release ready: $DMG"
