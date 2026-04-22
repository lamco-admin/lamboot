#!/bin/bash
# LamBoot Build Script

set -e

DIST_DIR="dist/EFI/LamBoot"
mkdir -p "$DIST_DIR/modules" "$DIST_DIR/reports" "$DIST_DIR/entries" "$DIST_DIR/drivers"

# Build x86_64
echo "Building LamBoot (x86_64)..."
cargo build --target x86_64-unknown-uefi --release -p lamboot-core
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
    cargo build --target aarch64-unknown-uefi --release -p lamboot-core 2>/dev/null
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
