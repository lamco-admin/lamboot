#!/bin/bash
# LamBoot Build Script

set -e

DIST_DIR="dist/EFI/LamBoot"
mkdir -p "$DIST_DIR/modules" "$DIST_DIR/reports" "$DIST_DIR/entries" "$DIST_DIR/drivers"

# Stage SBAT source from its tracked home (packaging/sbat.csv) into the
# build dist tree. sign-lamboot.sh reads dist/sbat.csv at sign time;
# packaging/sbat.csv is the source of truth tracked in git. Keeping
# both decoupled means dist/ can be wiped without losing the SBAT row,
# while sign-lamboot.sh keeps its existing dist/sbat.csv contract.
if [ -f packaging/sbat.csv ]; then
    cp packaging/sbat.csv dist/sbat.csv
    echo "  Staged: dist/sbat.csv (from packaging/sbat.csv)"
fi

# Build x86_64. --locked: the committed Cargo.lock is authoritative for the
# signed artifact, so a stale lock fails the build rather than silently
# resolving a different (possibly yanked-and-republished) dependency version.
echo "Building LamBoot (x86_64)..."
cargo build --locked --target x86_64-unknown-uefi --release -p lamboot-core
X64_OUT="target/x86_64-unknown-uefi/release/lambootx64.efi"
if [ -f "$X64_OUT" ]; then
    cp "$X64_OUT" "$DIST_DIR/"
    SIZE=$(stat -c %s "$X64_OUT" 2>/dev/null || stat -f %z "$X64_OUT")
    echo "  Built: lambootx64.efi ($(( SIZE / 1024 ))KB)"
fi

# Build diagnostic modules (x86_64)
echo "Building modules (x86_64)..."
cargo build --target x86_64-unknown-uefi --release -p nvme-diag -p mem-quick -p pci-inventory -p diag-shell 2>/dev/null || true
for mod_name in nvme-diag mem-quick pci-inventory diag-shell; do
    MOD_FILE="target/x86_64-unknown-uefi/release/${mod_name}.efi"
    if [ -f "$MOD_FILE" ]; then
        cp "$MOD_FILE" "$DIST_DIR/modules/"
        echo "  Built: modules/${mod_name}.efi"
    fi
done

# Build ARM64 (optional — skip if target not installed)
if rustup target list --installed | grep -q aarch64-unknown-uefi; then
    echo "Building LamBoot (aarch64)..."
    cargo build --locked --target aarch64-unknown-uefi --release -p lamboot-core 2>/dev/null
    AA64_OUT="target/aarch64-unknown-uefi/release/lambootx64.efi"
    if [ -f "$AA64_OUT" ]; then
        cp "$AA64_OUT" "$DIST_DIR/lambootaa64.efi"
        SIZE=$(stat -c %s "$AA64_OUT" 2>/dev/null || stat -f %z "$AA64_OUT")
        echo "  Built: lambootaa64.efi ($(( SIZE / 1024 ))KB)"
    fi
else
    echo "Skipping aarch64 (target not installed — run: rustup target add aarch64-unknown-uefi)"
fi

# Copy supporting files
cp examples/policy.toml "$DIST_DIR/" 2>/dev/null || true
cp lamboot-modules/manifest.toml "$DIST_DIR/modules/" 2>/dev/null || true

# Kernel hooks (Debian/Ubuntu postinst.d + postrm.d scripts) — install script
# expects these at dist/kernel-hooks/ relative to dist/EFI/LamBoot/, so parallel
# to the EFI/ tree in the release tarball.
if [ -d kernel-hooks ]; then
    mkdir -p dist/kernel-hooks
    cp kernel-hooks/zz-lamboot-postinst kernel-hooks/zz-lamboot-postrm dist/kernel-hooks/ 2>/dev/null || true
    # v0.11.13 — Proxmox-specific dpkg trigger (zzzz-lamboot-proxmox)
    cp kernel-hooks/zzzz-lamboot-proxmox dist/kernel-hooks/ 2>/dev/null || true
fi

# v0.15.0 — systemd kernel-install plugin (Arch/Fedora/openSUSE etc.). The
# installer reads it via find_dist_file("kernel-install/90-lamboot.install");
# without staging it here a build would ship only the Debian postinst.d hooks
# and systemd-kernel-install hosts would get no kernel-event integration.
if [ -d kernel-install ]; then
    mkdir -p dist/kernel-install
    cp kernel-install/90-lamboot.install dist/kernel-install/ 2>/dev/null || true
fi

# v0.11.13 — systemd units (source-of-truth in systemd/, staged into
# dist/systemd/ for the tarball pipeline). Pre-existing units in
# dist/systemd/ are left alone if a corresponding source isn't present.
if [ -d systemd ]; then
    mkdir -p dist/systemd
    cp systemd/*.service systemd/*.timer systemd/*.path dist/systemd/ 2>/dev/null || true
fi

# v0.11.13 — lamboot-host-sampler (Proxmox-host runtime metrics sampler)
if [ -f tools/lamboot-host-sampler ]; then
    cp tools/lamboot-host-sampler dist/lamboot-host-sampler
    chmod +x dist/lamboot-host-sampler
fi

# lamboot-kernel-hook — the BLS-entry generator the Debian/Ubuntu postinst.d
# hooks invoke after each apt kernel install. package-release.sh stages this
# next to lamboot-install in release tarballs; for ad-hoc deployments that
# extract dist/ into /usr/share/lamboot/, ship it there too so
# find_dist_file's "${SRC_DIR}/lamboot-kernel-hook" candidate resolves.
if [ -f tools/lamboot-kernel-hook ]; then
    cp tools/lamboot-kernel-hook dist/lamboot-kernel-hook
    chmod +x dist/lamboot-kernel-hook
fi

# Optional: sign all binaries for Secure Boot
if [ "${1:-}" = "--sign" ]; then
    echo ""
    echo "Signing binaries for Secure Boot..."
    if [ -z "${LAMBOOT_SIGN_KEY:-}" ]; then
        echo "Hint: to avoid a passphrase prompt per binary, run once before build:"
        echo "  source tools/sign-unlock"
        echo ""
    fi
    ./tools/sign-lamboot.sh
fi

echo ""
echo "Build complete!"
echo ""
echo "To install:"
echo "  1. Mount your ESP: mount /dev/sdXN /boot/efi"
echo "  2. Copy: cp -r dist/EFI/LamBoot /boot/efi/EFI/"
echo "  3. Add boot entry: efibootmgr -c -d /dev/sdX -p N -l '\\EFI\\LamBoot\\lambootx64.efi' -L 'LamBoot'"
echo ""
echo "To test: ./run-qemu.sh"
