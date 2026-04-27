#!/bin/bash
# Package LamBoot release tarball per docs/IMPLEMENTATION-PLAN.md §7.
#
# Outputs:
#   dist/lamboot-<VERSION>-<ARCH>.tar.gz
#   dist/lamboot-<VERSION>-<ARCH>.tar.gz.sha256
#
# Prerequisites:
#   - ./build.sh completed (signed + unsigned binaries present in dist/)
#   - ./tools/sign-lamboot.sh completed (signed drivers + modules present)
#   - dist/OVMF_VARS_lamboot.fd built with production keys
#   - Cargo.toml version matches the tarball version

set -euo pipefail

VERSION=$(awk -F'"' '/^version/ {print $2; exit}' Cargo.toml)
ARCH="x86_64"
STAGING="/tmp/lamboot-release-${VERSION}"
TARBALL_NAME="lamboot-${VERSION}-${ARCH}.tar.gz"
OUTPUT_DIR="${1:-dist}"

echo "══ Packaging LamBoot v${VERSION} (${ARCH}) ══"

# Sanity checks
REQUIRED=(
    dist/EFI/LamBoot/lambootx64.efi
    dist/EFI/LamBoot/lambootx64-signed.efi
    dist/EFI/LamBoot/db.der
    dist/EFI/LamBoot/drivers/ext4_x64.efi
    dist/EFI/LamBoot/drivers/ext4_x64-signed.efi
    dist/EFI/LamBoot/modules/manifest.toml
    dist/EFI/LamBoot/policy.toml
    dist/OVMF_VARS_lamboot.fd
    dist/kernel-hooks/zz-lamboot-postinst
    dist/kernel-hooks/zz-lamboot-postrm
    tools/lamboot-install
    CHANGELOG.md
    SECURITY.md
    LICENSE-MIT
    LICENSE-APACHE
    README.md
)

missing=0
for f in "${REQUIRED[@]}"; do
    if [[ ! -f "$f" ]]; then
        echo "❌ Missing: $f"
        missing=1
    fi
done
if (( missing )); then
    echo
    echo "Refusing to package. Fix missing artifacts first."
    exit 1
fi
echo "✓ All required artifacts present"

# Build staging tree
rm -rf "$STAGING"
mkdir -p "$STAGING/lamboot-${VERSION}"
cd "$STAGING/lamboot-${VERSION}"

# Install script + supporting tools
cp -a /home/greg/lamboot-dev/tools/lamboot-install ./lamboot-install
cp -a /home/greg/lamboot-dev/tools/lamboot-kernel-hook ./lamboot-kernel-hook
cp -a /home/greg/lamboot-dev/tools/lamboot-monitor.py ./lamboot-monitor.py
cp -a /home/greg/lamboot-dev/tools/sign-lamboot.sh ./sign-lamboot.sh
cp -a /home/greg/lamboot-dev/tools/sign-unlock ./sign-unlock
cp -a /home/greg/lamboot-dev/tools/sign-lock ./sign-lock
cp -a /home/greg/lamboot-dev/tools/build-ovmf-vars.sh ./build-ovmf-vars.sh 2>/dev/null || true

# lamboot-inspect — diagnostic suite (trust log / boot log / verify / dump).
# Bundled with the implementation package so the script can import
# lamboot_inspect.cli without an install step.
cp -a /home/greg/lamboot-dev/tools/lamboot-inspect ./lamboot-inspect
cp -a /home/greg/lamboot-dev/tools/lamboot_inspect ./lamboot_inspect
mkdir -p man/man1 completions
cp -a /home/greg/lamboot-dev/tools/lamboot-inspect.1 ./man/man1/lamboot-inspect.1
cp -a /home/greg/lamboot-dev/tools/completions/lamboot-inspect.bash ./completions/lamboot-inspect.bash
cp -a /home/greg/lamboot-dev/tools/completions/_lamboot-inspect ./completions/_lamboot-inspect

# EFI artifacts
mkdir -p EFI/LamBoot
cp -a /home/greg/lamboot-dev/dist/EFI/LamBoot/* EFI/LamBoot/

# OVMF VARS (Proxmox zero-touch, Config 4)
cp /home/greg/lamboot-dev/dist/OVMF_VARS_lamboot.fd ./OVMF_VARS_lamboot.fd

# Signing cert (public, for MOK enrollment by users)
cp /home/greg/lamboot-dev/dist/EFI/LamBoot/db.der ./lamboot-signing.der

# Kernel-install plugin + kernel hooks
mkdir -p kernel-install kernel-hooks
cp -a /home/greg/lamboot-dev/dist/kernel-install/* kernel-install/ 2>/dev/null || true
cp -a /home/greg/lamboot-dev/dist/kernel-hooks/* kernel-hooks/ 2>/dev/null || true

# Systemd
mkdir -p systemd
cp -a /home/greg/lamboot-dev/dist/systemd/* systemd/ 2>/dev/null || true

# Documentation
mkdir -p docs
cp /home/greg/lamboot-dev/CHANGELOG.md ./CHANGELOG.md
cp /home/greg/lamboot-dev/SECURITY.md ./SECURITY.md
cp /home/greg/lamboot-dev/README.md ./README.md
cp /home/greg/lamboot-dev/LICENSE-MIT ./LICENSE-MIT
cp /home/greg/lamboot-dev/LICENSE-APACHE ./LICENSE-APACHE

# User-facing documentation (NOT internal analysis)
for d in \
    SECURE-BOOT-DEPLOYMENT.md \
    SECURITY-MODEL.md \
    MOK-ENROLLMENT-GUIDE.md \
    OVMF-VARS-PROXMOX.md \
    PROXMOX-GUIDE.md \
    KEY-GENERATION.md \
    INSTALL-REFERENCE.md \
    CONFIGURATION-GUIDE.md \
    TROUBLESHOOTING-GUIDE.md \
    USER-GUIDE.md \
    SECURITY-GUIDE.md \
    DIAGNOSTIC-MODULES.md \
    ARCHITECTURE.md \
    LAMBOOT-TOOLS-OVERVIEW.md \
    LAMBOOT-INSPECT.md \
    ROADMAP.md; do
    if [[ -f "/home/greg/lamboot-dev/docs/$d" ]]; then
        cp "/home/greg/lamboot-dev/docs/$d" "docs/$d"
    fi
done

# Checksum manifest for reproducibility
echo "══ Writing manifest ══"
find . -type f ! -name MANIFEST.sha256 | sort | xargs sha256sum > MANIFEST.sha256
echo "✓ MANIFEST.sha256 ($(wc -l < MANIFEST.sha256) files)"

# Tar it
cd "$STAGING"
ABSOLUTE_OUTPUT="/home/greg/lamboot-dev/$OUTPUT_DIR"
mkdir -p "$ABSOLUTE_OUTPUT"
echo
echo "══ Creating tarball ══"
tar czf "${ABSOLUTE_OUTPUT}/${TARBALL_NAME}" "lamboot-${VERSION}/"
ls -la "${ABSOLUTE_OUTPUT}/${TARBALL_NAME}"

# SHA256
sha256sum "${ABSOLUTE_OUTPUT}/${TARBALL_NAME}" | tee "${ABSOLUTE_OUTPUT}/${TARBALL_NAME}.sha256"

# Cleanup staging
rm -rf "$STAGING"

echo
echo "✓ Release tarball ready: ${ABSOLUTE_OUTPUT}/${TARBALL_NAME}"
echo "  SHA256: $(awk '{print $1}' ${ABSOLUTE_OUTPUT}/${TARBALL_NAME}.sha256)"
echo
echo "Next steps:"
echo "  1. Tag: git tag -a v${VERSION} -m 'LamBoot v${VERSION}'"
echo "  2. Push tag: git push origin v${VERSION}"
echo "  3. Run the project's publish workflow to stage and ship the release."
