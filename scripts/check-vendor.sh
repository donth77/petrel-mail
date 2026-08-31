#!/usr/bin/env bash
# Is vendor/imap-proto still the crate it claims to be?
#
# The tree carries a patched copy of imap-proto because the published one
# refuses 8-bit quoted-strings, which breaks every account whose folders or
# subjects are not ASCII (docs 17). Vendoring buys that fix and costs the
# thing a registry dependency gives for free: the lockfile entry has no
# `source` and no `checksum`, so nothing would notice if those six thousand
# lines quietly became six thousand and one.
#
# This is that notice. It downloads the release from crates.io and requires
# every file to be byte-identical except four:
#
#   Cargo.toml, Cargo.lock              vendoring changes these by necessity
#   src/parser/core.rs                  the one-line is_char relaxation
#   src/parser/rfc3501/mod.rs           its test
#
# and pins the two source files by hash, so even they cannot drift unnoticed.
# Changing the patch on purpose means updating the hashes below in the same
# commit, which is exactly the moment somebody should be looking.
#
# Drop this script, the vendor directory and the [patch.crates-io] stanza
# together when imap-proto 0.17 is on crates.io and async-imap depends on it.

set -euo pipefail

CRATE=imap-proto
VERSION=0.16.7
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VENDOR="$ROOT/vendor/$CRATE"

# The vendored copies of the two files we mean to have changed. Update these
# in the same commit as any deliberate change to the patch.
EXPECT_CORE=decc47f950015e99a187093d620e216f88e385f55bb047d5fef3179c0a82238a
EXPECT_RFC3501=515bac48d3985dd60fb1a5b63af518492d980fef4d7657ebd4ab3ada8fc65573

fail() {
    echo "vendor check FAILED: $*" >&2
    exit 1
}

[ -d "$VENDOR" ] || fail "$VENDOR is missing"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

echo "fetching $CRATE $VERSION from crates.io…"
curl -fsSL -o "$work/c.crate" \
    "https://static.crates.io/crates/$CRATE/$CRATE-$VERSION.crate" \
    || fail "could not download $CRATE $VERSION"
tar xzf "$work/c.crate" -C "$work"
UP="$work/$CRATE-$VERSION"

# Cargo.toml.orig is an artefact of packaging and is not in a vendored tree.
allowed_to_differ="Cargo.toml Cargo.lock Cargo.toml.orig src/parser/core.rs src/parser/rfc3501/mod.rs"

status=0
# Walk both sides so an added file is caught as loudly as a changed one.
while read -r rel; do
    case " $allowed_to_differ " in *" $rel "*) continue ;; esac
    if [ ! -f "$VENDOR/$rel" ]; then
        echo "  missing from vendor: $rel" >&2
        status=1
    elif ! cmp -s "$UP/$rel" "$VENDOR/$rel"; then
        echo "  differs from upstream: $rel" >&2
        status=1
    fi
done < <(cd "$UP" && find . -type f | sed 's|^\./||' | sort)

while read -r rel; do
    case " $allowed_to_differ " in *" $rel "*) continue ;; esac
    if [ ! -f "$UP/$rel" ]; then
        echo "  not in upstream at all: $rel" >&2
        status=1
    fi
done < <(cd "$VENDOR" && find . -type f | sed 's|^\./||' | sort)

[ "$status" -eq 0 ] || fail "the vendored crate has changes beyond the intended patch"

check_hash() {
    local rel="$1" want="$2" got
    got="$(shasum -a 256 "$VENDOR/$rel" | cut -d' ' -f1)"
    [ "$got" = "$want" ] || fail "$rel changed
  expected $want
  found    $got
  If the patch changed on purpose, update the hash in $(basename "$0")."
}

check_hash src/parser/core.rs "$EXPECT_CORE"
check_hash src/parser/rfc3501/mod.rs "$EXPECT_RFC3501"

echo "vendor check ok: $CRATE $VERSION, patched only where intended"
