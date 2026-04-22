#!/bin/bash
# Run LamBoot in QEMU with OVMF

set -e

# Check if OVMF is available
OVMF_CODE="/usr/share/OVMF/OVMF_CODE.fd"
OVMF_VARS="/usr/share/OVMF/OVMF_VARS.fd"

if [ ! -f "$OVMF_CODE" ]; then
    echo "OVMF firmware not found. Please install:"
    echo "  Ubuntu/Debian: apt install ovmf"
    echo "  Fedora: dnf install edk2-ovmf"
    echo "  Arch: pacman -S edk2-ovmf"
    exit 1
fi

# Build first
./build.sh

# Create temporary VARS file
TEMP_VARS=$(mktemp)
cp "$OVMF_VARS" "$TEMP_VARS"

# Create ESP disk image
ESP_IMG="target/esp.img"
dd if=/dev/zero of="$ESP_IMG" bs=1M count=64 2>/dev/null
mkfs.vfat "$ESP_IMG" > /dev/null 2>&1

# Mount and copy files
MOUNT_DIR=$(mktemp -d)
sudo mount "$ESP_IMG" "$MOUNT_DIR"
sudo mkdir -p "$MOUNT_DIR/EFI/BOOT"
sudo cp dist/EFI/LamBoot/lambootx64.efi "$MOUNT_DIR/EFI/BOOT/BOOTX64.EFI"
sudo mkdir -p "$MOUNT_DIR/EFI/LamBoot"
sudo cp -r dist/EFI/LamBoot/* "$MOUNT_DIR/EFI/LamBoot/"
sudo umount "$MOUNT_DIR"
rmdir "$MOUNT_DIR"

echo "Starting QEMU..."
echo "Press Ctrl+Alt+G to release mouse/keyboard"
echo ""

qemu-system-x86_64 \
    -enable-kvm \
    -m 512 \
    -drive if=pflash,format=raw,readonly=on,file="$OVMF_CODE" \
    -drive if=pflash,format=raw,file="$TEMP_VARS" \
    -drive format=raw,file="$ESP_IMG" \
    -net none \
    -vga std \
    -serial stdio

rm "$TEMP_VARS"

echo ""
echo "QEMU exited"
