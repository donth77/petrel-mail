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

# ------------------------------------------------------- update artifact
say "update artifact"
# The updater installs a tarball of the .app, not the DMG, and verifies its
# signature against the public key compiled into the running copy. The
# private key never enters this repository: it lives at
# ~/.config/petrel/updater.key (or wherever TAURI_SIGNING_PRIVATE_KEY_PATH
# points), and losing it means no existing install can ever be updated
# again — back it up somewhere you would keep an SSH key.
KEY_PATH="${TAURI_SIGNING_PRIVATE_KEY_PATH:-$HOME/.config/petrel/updater.key}"
TARBALL="$ROOT/target/release/Petrel-$VERSION.app.tar.gz"
if [ -f "$KEY_PATH" ] && command -v cargo-tauri >/dev/null; then
  # COPYFILE_DISABLE=1 is load-bearing. Without it macOS tar stores each
  # file's extended attributes as a second, parallel "._name" member —
  # half the archive — and the updater's Rust tar reader has no special
  # case for them: it tries to unpack "._Petrel.app" as a bundle and
  # fails. The update then downloads, verifies its signature, and refuses
  # to install, which reads as a corrupt release rather than a packaging
  # bug. macOS tar also hides these members from its own `tar tzf`, so
  # the archive looks right until a real install unpacks it.
  (cd "$(dirname "$APP")" && COPYFILE_DISABLE=1 tar czf "$TARBALL" "$(basename "$APP")")
  TAURI_SIGNING_PRIVATE_KEY_PATH="$KEY_PATH" \
  TAURI_SIGNING_PRIVATE_KEY_PASSWORD="${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}" \
    cargo tauri signer sign "$TARBALL" >/dev/null
  SIGNATURE="$(cat "$TARBALL.sig")"
  NOTES="${PETREL_RELEASE_NOTES:-}"
  PUBDATE="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  cat > "$ROOT/target/release/latest.json" <<JSON
{
  "version": "$VERSION",
  "notes": "$NOTES",
  "pub_date": "$PUBDATE",
  "platforms": {
    "darwin-aarch64": {
      "signature": "$SIGNATURE",
      "url": "https://github.com/donth77/petrel-mail/releases/download/v$VERSION/Petrel-$VERSION.app.tar.gz"
    },
    "darwin-x86_64": {
      "signature": "$SIGNATURE",
      "url": "https://github.com/donth77/petrel-mail/releases/download/v$VERSION/Petrel-$VERSION.app.tar.gz"
    }
  }
}
JSON
  echo "signed tarball: $TARBALL"
  echo "manifest:       $ROOT/target/release/latest.json"
else
  echo "no update-signing key at $KEY_PATH — skipping the update artifact." >&2
  echo "Existing installs will not see this release. Generate one with:" >&2
  echo "  cargo tauri signer generate -w $KEY_PATH" >&2
  echo "and put the printed public key in tauri.conf.json under plugins.updater." >&2
fi

printf '\n%s\n' "release ready: $DMG"
if [ -f "$ROOT/target/release/latest.json" ]; then
  cat <<'NEXT'

To publish: create the GitHub release for this version and attach both the
.dmg (what a person downloads) and the .app.tar.gz plus latest.json (what
existing installs read). The endpoint in tauri.conf.json points at
releases/latest/download/latest.json, so the manifest must be an asset of
the release marked latest — not a file in the repository.
NEXT
fi
