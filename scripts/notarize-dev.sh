#!/usr/bin/env bash
# Notarize the *current* development build, so notifications can be tested.
#
# Not a release. No DMG, no tag, no version bump — this takes the app that
# `rebuild.sh` just produced, sends it to Apple, staples the ticket, and hands
# it back. Everything else about the working tree is left alone.
#
# It exists because of one macOS rule that has no local workaround: the system
# refuses notification authorization to a build it has not accepted through
# Gatekeeper. `requestAuthorization` comes back UNErrorDomain 1 — "Notifications
# are not allowed for this application" — and it never shows the user a prompt,
# so a locally signed build looks broken when it is merely untrusted. Signing
# alone is not enough; notarization is what changes the verdict.
#
# Minutes, not seconds: Apple's service is the slow part. This is a thing to
# run before a release, or when the notification path itself has changed —
# not on every rebuild.
#
#   ./scripts/rebuild.sh && ./scripts/notarize-dev.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

APP="$ROOT/target/Petrel.app"
IDENTITY="${PETREL_SIGN_IDENTITY:-Developer ID Application: Thomas Donohue (7726PJ7MGW)}"
NOTARY_PROFILE="${PETREL_NOTARY_PROFILE:-petrel-notary}"
ENTITLEMENTS="$ROOT/apps/desktop/src-tauri/entitlements.plist"

say() { printf '\n\033[1m==> %s\033[0m\n' "$1"; }
die() { printf '\033[31m%s\033[0m\n' "$1" >&2; exit 1; }

[ -d "$APP" ] || die "no app bundle. Run ./scripts/rebuild.sh first."
xcrun notarytool history --keychain-profile "$NOTARY_PROFILE" >/dev/null 2>&1 ||
  die "no notarization credential in profile '$NOTARY_PROFILE'. See scripts/release.sh."

# The hardened runtime and a timestamp are both refusal reasons if missing, and
# rebuild.sh does not set them — it signs for local running, not for Apple.
say "re-sign with the hardened runtime"
if [ -f "$ENTITLEMENTS" ]; then
  codesign --force --deep --options runtime --timestamp \
    --entitlements "$ENTITLEMENTS" --sign "$IDENTITY" "$APP"
else
  codesign --force --deep --options runtime --timestamp --sign "$IDENTITY" "$APP"
fi
codesign --verify --deep --strict "$APP"

# notarytool takes an archive, not a bundle. A zip is the cheapest container
# and nothing downstream ever sees it — the ticket is stapled to the .app.
say "submit to Apple (a few minutes is normal)"
ZIP="$(mktemp -d)/Petrel.zip"
ditto -c -k --keepParent "$APP" "$ZIP"
xcrun notarytool submit "$ZIP" --keychain-profile "$NOTARY_PROFILE" --wait

say "staple"
xcrun stapler staple "$APP"
xcrun stapler validate "$APP"

# The verdict that actually matters, and the one that decides whether macOS
# will grant this build the right to post a notification.
say "what Gatekeeper says now"
spctl -a -vv "$APP"

printf '\nNotarized: %s\n' "$APP"
printf 'Now: ./scripts/run-demo.sh, then Settings > Notifications > send a test.\n'
