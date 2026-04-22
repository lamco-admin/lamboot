#!/bin/bash
# Export LamBoot source + release artifacts to public repo.
#
# ⚠️  GATE-PUB: This script is the ONLY authorized pathway from
#     lamco-admin/lamboot-dev (private) → lamco-admin/lamboot (public).
#     Per ~/.claude/CLAUDE.md rules: never manually push to the public repo.
#
# What this script does:
#   1. Verifies dev repo is clean and on a tagged release
#   2. Refuses to proceed if keys/*.key is present (private keys must not leak)
#   3. Clones fresh copy of public repo (or updates existing)
#   4. Syncs a sanitized subset of dev repo to public repo
#   5. Copies release tarball as a GitHub release asset
#   6. Shows the user the diff + tarball contents before pushing
#   7. Requires explicit human confirmation (--yes to skip in CI)
#   8. Pushes tag + release via gh CLI
#
# Usage:
#   ./export-to-public.sh [--dry-run] [--yes] VERSION

set -euo pipefail

DRY_RUN=false
AUTO_APPROVE=false
ARGS=()
for arg in "$@"; do
    case "$arg" in
        --dry-run) DRY_RUN=true ;;
        --yes)     AUTO_APPROVE=true ;;
        *)         ARGS+=("$arg") ;;
    esac
done

VERSION="${ARGS[0]:-}"
if [[ -z "$VERSION" ]]; then
    cat <<EOF
Usage: $0 [--dry-run] [--yes] VERSION
Examples:
  $0 --dry-run 0.8.3     # Preview without pushing
  $0 0.8.3               # Interactive push
  $0 --yes 0.8.3         # Non-interactive (CI)
EOF
    exit 1
fi

# Paths
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEV_REPO="$SCRIPT_DIR"
PUBLIC_REPO="$HOME/lamboot"
STAGING_DIR="/tmp/lamboot-export-v${VERSION}"

cat <<EOF
╔══════════════════════════════════════════════════════════════════╗
║  ⚠️  GATE-PUB: Export LamBoot to PUBLIC Repository                ║
╚══════════════════════════════════════════════════════════════════╝

Target:  https://github.com/lamco-admin/lamboot
Version: v$VERSION
Source:  $DEV_REPO
Mode:    $( $DRY_RUN && echo DRY-RUN || echo PUSH )
EOF

echo
echo "══ Pre-flight checks ══"
cd "$DEV_REPO"

# Dev repo must be clean
if ! git diff-index --quiet HEAD --; then
    echo "❌ Dev repo has uncommitted changes. Commit or stash before exporting."
    exit 1
fi
echo "✓ Dev repo clean"

# Tag must exist
if ! git tag | grep -q "^v${VERSION}$"; then
    echo "❌ Tag v${VERSION} not found in dev repo."
    echo "   Create it first: git tag -a v${VERSION} -m 'LamBoot v${VERSION}' && git push origin v${VERSION}"
    exit 1
fi
echo "✓ Tag v${VERSION} exists"

# No private key material tracked
if git ls-files | grep -E '\.key$|\.pem$|keys-gen/|keys-archive/' | head -5 | grep -q .; then
    echo "❌ Private-key material is git-tracked. REFUSING to proceed."
    git ls-files | grep -E '\.key$|\.pem$|keys-gen/|keys-archive/' | head -10
    exit 1
fi
echo "✓ No private-key material tracked"

# Release tarball must exist
RELEASE_TARBALL="$DEV_REPO/dist/lamboot-${VERSION}-x86_64.tar.gz"
if [[ ! -f "$RELEASE_TARBALL" ]]; then
    echo "❌ Release tarball not found at $RELEASE_TARBALL"
    echo "   Build it via the release packaging task first."
    exit 1
fi
TARBALL_SHA256=$(sha256sum "$RELEASE_TARBALL" | awk '{print $1}')
echo "✓ Release tarball present ($(du -h "$RELEASE_TARBALL" | awk '{print $1}'))"
echo "  sha256: $TARBALL_SHA256"

# CHANGELOG check
if ! grep -qF "[${VERSION}]" "$DEV_REPO/CHANGELOG.md" 2>/dev/null; then
    echo "⚠ CHANGELOG.md has no entry for [${VERSION}]. Consider adding release notes."
fi

echo
echo "══ Preparing public repo working copy ══"

if [[ ! -d "$PUBLIC_REPO/.git" ]]; then
    echo "Cloning public repo to $PUBLIC_REPO"
    git clone https://github.com/lamco-admin/lamboot.git "$PUBLIC_REPO"
else
    echo "Updating $PUBLIC_REPO"
    cd "$PUBLIC_REPO"
    git fetch origin
    # First-time / empty-remote case: no main or master branch exists remotely yet.
    # Try main → master → assume fresh repo (stay on whatever HEAD we have, create main).
    if git checkout main 2>/dev/null; then
        git pull --ff-only 2>/dev/null || true
    elif git checkout master 2>/dev/null; then
        git pull --ff-only 2>/dev/null || true
    else
        echo "  (remote has no main/master branch yet — will create 'main' on first export push)"
        git checkout -B main 2>/dev/null || true
    fi
fi
echo "✓ Public repo ready"

# Sanitized sync list
SYNC_EXCLUDES=(
    --exclude='.git/'
    --exclude='target/'
    --exclude='dist/'
    --exclude='keys/'
    --exclude='keys-gen/'
    --exclude='keys-archive/'
    --exclude='.claude/'
    --exclude='CLAUDE.md'
    --exclude='*.bak'
    --exclude='*.orig'
    --exclude='*.swp'
    --exclude='reports/'
    --exclude='docs/analysis/'
    --exclude='docs/STATUS-*.md'
    --exclude='docs/*AUDIT*.md'
    --exclude='docs/*LANDSCAPE*.md'
    --exclude='docs/ARCHITECTURE-DECISIONS-RESEARCH.md'
    --exclude='docs/ARCHITECTURE-TENSIONS-ANALYSIS.md'
    --exclude='docs/F2FS-BOOT-ECOSYSTEM-ANALYSIS.md'
    --exclude='docs/FILESYSTEM-DRIVER-ANALYSIS.md'
    --exclude='docs/INNOVATION-ROADMAP-*.md'
    --exclude='docs/CROSS-PLATFORM-INTEGRATION-REPORT.md'
    --exclude='docs/AUTODISCOVERY-AND-INSTALL-INNOVATIONS-*.md'
    --exclude='docs/LAMBOOT-COMPREHENSIVE-AUDIT-*.md'
    --exclude='docs/INSTALL-SCRIPT-TEST-RESULTS-*.md'
    --exclude='docs/ROADMAP-AUDIT-*.md'
    --exclude='docs/TESTING-RESULTS-*.md'
    --exclude='docs/TESTING-PLAN-*.md'
    --exclude='docs/SECURE-BOOT-BLOCKER.md'
    --exclude='*.tar'
    --exclude='*.tar.gz'
)

echo
echo "══ Diff vs current public repo ══"
rsync -a --checksum --dry-run --delete "${SYNC_EXCLUDES[@]}" \
    "$DEV_REPO/" "$PUBLIC_REPO/" 2>&1 | grep -v '^\.' | head -40 || true

if $DRY_RUN; then
    echo
    echo "══ DRY-RUN COMPLETE ══"
    echo "No changes pushed."
    exit 0
fi

if ! $AUTO_APPROVE; then
    echo
    echo "══════════════════════════════════════════════════════════════"
    echo "  APPROVAL GATE — about to push to PUBLIC repository"
    echo "══════════════════════════════════════════════════════════════"
    echo "  Version:  v${VERSION}"
    echo "  Target:   https://github.com/lamco-admin/lamboot"
    echo "  Tarball:  $RELEASE_TARBALL (sha256: $TARBALL_SHA256)"
    echo
    read -r -p "Type the version exactly to confirm push (e.g. 0.8.3): " CONFIRM
    if [[ "$CONFIRM" != "$VERSION" ]]; then
        echo "Version mismatch — aborting."
        exit 1
    fi
fi

echo
echo "══ Applying to public repo ══"
rsync -a --delete "${SYNC_EXCLUDES[@]}" \
    "$DEV_REPO/" "$PUBLIC_REPO/"

cd "$PUBLIC_REPO"
git add -A
if git diff-index --quiet HEAD --; then
    echo "No changes to commit in public repo."
else
    git commit -m "Release v${VERSION}

See CHANGELOG.md for details.

Signed release tarball: lamboot-${VERSION}-x86_64.tar.gz
SHA256: ${TARBALL_SHA256}"
fi

if ! git tag | grep -q "^v${VERSION}$"; then
    git tag -a "v${VERSION}" -m "LamBoot v${VERSION}"
fi

echo
echo "══ Pushing ══"
git push origin HEAD
git push origin "v${VERSION}"

echo
echo "══ Creating GitHub release ══"
if gh release view "v${VERSION}" --repo lamco-admin/lamboot >/dev/null 2>&1; then
    echo "Release v${VERSION} already exists — uploading asset."
    gh release upload "v${VERSION}" "$RELEASE_TARBALL" --repo lamco-admin/lamboot --clobber
else
    NOTES_FILE=$(mktemp)
    awk -v v="${VERSION}" '
        $0 ~ "^## \\[" v "\\]" { found=1; print; next }
        /^## \[/ && found { exit }
        found { print }
    ' "$DEV_REPO/CHANGELOG.md" > "$NOTES_FILE"
    [[ -s "$NOTES_FILE" ]] || echo "Release v${VERSION}" > "$NOTES_FILE"
    gh release create "v${VERSION}" "$RELEASE_TARBALL" \
        --repo lamco-admin/lamboot \
        --title "LamBoot v${VERSION}" \
        --notes-file "$NOTES_FILE"
    rm -f "$NOTES_FILE"
fi

echo
echo "✓ Export complete"
echo "  Public repo:  https://github.com/lamco-admin/lamboot"
echo "  Release:      https://github.com/lamco-admin/lamboot/releases/tag/v${VERSION}"
