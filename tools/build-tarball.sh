#!/bin/bash
# build-tarball.sh — assemble a release tarball matching the v0.8.4 layout.
#
# Inputs (must already be built + signed):
#   - dist/EFI/LamBoot/  (binary, drivers, modules, policy)
#   - dist/OVMF_VARS_lamboot.fd
#   - dist/sbat.csv
#   - dist/kernel-install/, dist/kernel-hooks/, dist/systemd/
#   - tools/lamboot-install, tools/lamboot-kernel-hook, tools/sign-*
#   - tools/lamboot-inspect (+ sibling lamboot_inspect/ pkg)
#   - tools/build-ovmf-vars.sh, tools/lamboot-monitor.py
#   - keys/db.der renamed to lamboot-signing.der
#   - top-level licenses + README + CHANGELOG + SECURITY
#   - docs/* curated subset
#
# Output:
#   dist/lamboot-<version>-x86_64.tar.gz
#   dist/lamboot-<version>-x86_64.tar.gz.sha256
#
# Uses the workspace version from Cargo.toml as the version string.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

VERSION=$(grep -E '^version = ' Cargo.toml | head -1 | cut -d'"' -f2)
[[ -n "$VERSION" ]] || { echo "could not read version from Cargo.toml" >&2; exit 1; }

ARCH="x86_64"
STAGE_NAME="lamboot-${VERSION}"
STAGE_DIR="dist/${STAGE_NAME}"
TARBALL="dist/lamboot-${VERSION}-${ARCH}.tar.gz"
SHA="${TARBALL}.sha256"

echo "=== Building lamboot-${VERSION}-${ARCH}.tar.gz ==="

# Clean previous staging
rm -rf "$STAGE_DIR" "$TARBALL" "$SHA"

mkdir -p "$STAGE_DIR"

# 1. Top-level licenses + README + CHANGELOG + SECURITY
cp LICENSE-APACHE LICENSE-MIT README.md SECURITY.md CHANGELOG.md "$STAGE_DIR/"

# 2. Curated docs (matches v0.8.4 selection)
mkdir -p "$STAGE_DIR/docs"
for d in ARCHITECTURE CONFIGURATION-GUIDE DIAGNOSTIC-MODULES INSTALL-REFERENCE \
         KEY-GENERATION LAMBOOT-INSPECT LAMBOOT-TOOLS-OVERVIEW MOK-ENROLLMENT-GUIDE \
         OVMF-VARS-PROXMOX PROXMOX-GUIDE ROADMAP SECURE-BOOT-DEPLOYMENT \
         SECURITY-GUIDE SECURITY-MODEL TROUBLESHOOTING-GUIDE USER-GUIDE; do
    [[ -f "docs/${d}.md" ]] && cp "docs/${d}.md" "$STAGE_DIR/docs/"
done

# 3. EFI/ tree (binaries + drivers + modules + policy)
mkdir -p "$STAGE_DIR/EFI"
cp -r dist/EFI/LamBoot "$STAGE_DIR/EFI/"
# Keep an empty reports/ placeholder so install scripts can write into it
mkdir -p "$STAGE_DIR/EFI/LamBoot/reports"

# 4. systemd + kernel-install + kernel-hooks
[[ -d dist/systemd ]] && cp -r dist/systemd "$STAGE_DIR/"
[[ -d dist/kernel-install ]] && cp -r dist/kernel-install "$STAGE_DIR/"
[[ -d dist/kernel-hooks ]] && cp -r dist/kernel-hooks "$STAGE_DIR/"

# 5. OVMF_VARS template (Proxmox)
[[ -f dist/OVMF_VARS_lamboot.fd ]] && cp dist/OVMF_VARS_lamboot.fd "$STAGE_DIR/"
[[ -f dist/sbat.csv ]] && cp dist/sbat.csv "$STAGE_DIR/"

# 6. Tools — install + kernel-hook + sign scripts + monitor + ovmf builder
cp tools/lamboot-install "$STAGE_DIR/"
[[ -f tools/lamboot-kernel-hook ]] && cp tools/lamboot-kernel-hook "$STAGE_DIR/"
cp tools/sign-lamboot.sh "$STAGE_DIR/"
cp tools/sign-lock "$STAGE_DIR/"
cp tools/sign-unlock "$STAGE_DIR/"
[[ -f tools/build-ovmf-vars.sh ]] && cp tools/build-ovmf-vars.sh "$STAGE_DIR/"
[[ -f tools/lamboot-monitor.py ]] && cp tools/lamboot-monitor.py "$STAGE_DIR/"

# 7. lamboot-inspect (Python tool + package)
[[ -f tools/lamboot-inspect ]] && cp tools/lamboot-inspect "$STAGE_DIR/"
[[ -d tools/lamboot_inspect ]] && cp -r tools/lamboot_inspect "$STAGE_DIR/"
# Strip __pycache__ if rsynced
find "$STAGE_DIR" -name '__pycache__' -type d -exec rm -rf {} + 2>/dev/null || true

# 8. Completions + man pages
[[ -d tools/completions ]] && cp -r tools/completions "$STAGE_DIR/"
mkdir -p "$STAGE_DIR/man/man1"
[[ -f tools/lamboot-inspect.1 ]] && cp tools/lamboot-inspect.1 "$STAGE_DIR/man/man1/"

# 9. Public signing certificate (DER format) — for users who want to verify
#    the binary against the LamBoot Release Signing Key.
[[ -f keys/db.der ]] && cp keys/db.der "$STAGE_DIR/lamboot-signing.der"

# 10. lib/ — esp-deploy.sh canonical lib (consumed by lamboot-tools mirror)
mkdir -p "$STAGE_DIR/lib"
[[ -f lib/esp-deploy.sh ]] && cp lib/esp-deploy.sh "$STAGE_DIR/lib/"

# 11. MANIFEST.sha256 — list every file with sha256 for tamper detection
echo "Generating MANIFEST.sha256..."
( cd "$STAGE_DIR" && \
  find . -type f ! -name 'MANIFEST.sha256' -print0 \
    | sort -z \
    | xargs -0 sha256sum \
    | sed 's| \./| |' \
    > MANIFEST.sha256 )

# 12. Tarball
echo "Building tarball..."
tar --owner=0 --group=0 --numeric-owner \
    -czf "$TARBALL" -C dist "$STAGE_NAME"

# 13. Tarball SHA256
( cd dist && sha256sum "$(basename "$TARBALL")" ) > "$SHA"

# 14. Cleanup staging
rm -rf "$STAGE_DIR"

echo
echo "Tarball:    $TARBALL"
echo "SHA256:     $SHA"
echo
ls -la "$TARBALL" "$SHA"
echo
cat "$SHA"
