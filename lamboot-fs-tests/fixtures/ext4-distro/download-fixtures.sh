#!/bin/bash
# download-fixtures.sh — fetch SDS-2 distro ext4 fixtures from hosting
#
# Mirror of the tools-dev pattern
# (lamboot-tools-dev/tests/fixtures/download-fixtures.sh). Fetches the
# `*.raw.zst` files listed in `fixtures.sha256` from one of:
#
#   FIXTURES_SSH_HOST=<host>   — scp from /var/lib/lamboot-fixtures/ext4-distro/
#                                (default within lamco infra)
#   FIXTURES_LOCAL_DIR=<dir>   — copy from a local staging dir
#   FIXTURES_BASE_URL=<url>    — HTTPS fetch (future public hosting)
#
# Re-run is idempotent: already-present fixtures with matching SHA are
# skipped. Required by `run-qemu-ext4-backend-test.sh` and the host
# test at `lamboot-fs-tests/tests/ext4.rs` (which uses the upstream
# bundled fixture, not these — those are deliberately lightweight and
# ship in-repo).

set -uo pipefail

FIXTURES_SSH_HOST="${FIXTURES_SSH_HOST:-}"
FIXTURES_SSH_PATH="${FIXTURES_SSH_PATH:-/var/lib/lamboot-fixtures/ext4-distro}"
FIXTURES_LOCAL_DIR="${FIXTURES_LOCAL_DIR:-}"
FIXTURES_BASE_URL="${FIXTURES_BASE_URL:-https://fixtures.lamboot.dev/ext4-distro}"
FIXTURES_DIR="$(cd "$(dirname "$0")" && pwd)"
CHECKSUMS="$FIXTURES_DIR/fixtures.sha256"

readonly RED=$'\033[0;31m'
readonly YELLOW=$'\033[0;33m'
readonly GREEN=$'\033[0;32m'
readonly RESET=$'\033[0m'

fail() { printf '%s[fail]%s %s\n' "$RED" "$RESET" "$1" >&2; exit 1; }
warn() { printf '%s[warn]%s %s\n' "$YELLOW" "$RESET" "$1" >&2; }
ok()   { printf '%s[ok]%s %s\n' "$GREEN" "$RESET" "$1"; }

[ -f "$CHECKSUMS" ] || fail "checksums file not found: $CHECKSUMS"

total=0
skipped=0
fetched=0
failed=0

while IFS= read -r line; do
    [ -z "$line" ] && continue
    [[ "$line" =~ ^# ]] && continue
    sha=${line%% *}
    name=${line##* }
    [ -z "$sha" ] || [ -z "$name" ] && continue
    total=$((total + 1))
    path="$FIXTURES_DIR/$name"

    if [ -f "$path" ]; then
        have=$(sha256sum "$path" | awk '{print $1}')
        if [ "$have" = "$sha" ]; then
            skipped=$((skipped + 1))
            continue
        fi
        warn "checksum mismatch on $name — refetching"
        rm -f "$path"
    fi

    if [ -n "$FIXTURES_SSH_HOST" ]; then
        if scp -q "$FIXTURES_SSH_HOST:$FIXTURES_SSH_PATH/$name" "$path"; then
            fetched=$((fetched + 1))
        else
            failed=$((failed + 1))
            warn "scp from $FIXTURES_SSH_HOST failed for $name"
            continue
        fi
    elif [ -n "$FIXTURES_LOCAL_DIR" ]; then
        if cp -f "$FIXTURES_LOCAL_DIR/$name" "$path"; then
            fetched=$((fetched + 1))
        else
            failed=$((failed + 1))
            continue
        fi
    else
        if curl -fsSL -o "$path" "$FIXTURES_BASE_URL/$name"; then
            fetched=$((fetched + 1))
        else
            failed=$((failed + 1))
            warn "HTTPS fetch failed for $name"
            continue
        fi
    fi

    have=$(sha256sum "$path" | awk '{print $1}')
    if [ "$have" != "$sha" ]; then
        warn "checksum mismatch AFTER fetch: $name (got $have expected $sha)"
        rm -f "$path"
        failed=$((failed + 1))
    fi
done < "$CHECKSUMS"

printf '\n%s[summary]%s %d total, %d skipped, %d fetched, %d failed\n' \
    "$GREEN" "$RESET" "$total" "$skipped" "$fetched" "$failed"
[ "$failed" -eq 0 ] || exit 1
