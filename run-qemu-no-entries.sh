#!/bin/bash
# Task #40 — no-entries recovery deliberate test.
#
# Builds an ESP that has LamBoot but deliberately NO BLS entries and NO UKIs.
# LamBoot should:
#   - discover zero bootable entries
#   - render the "no bootable entries found" recovery screen
#   - offer: F2 (Reboot to Firmware Setup), F12 (Cold Reboot), Power-off
#
# Watch serial output. Exit with Ctrl+A then X.

set -e

./build.sh >/dev/null 2>&1 || { echo "build failed"; exit 1; }

ESP_IMG="target/esp-no-entries.img"
TEMP_VARS=$(mktemp --suffix=.fd)
cp /usr/share/OVMF/OVMF_VARS_4M.fd "$TEMP_VARS"
trap "rm -f $TEMP_VARS" EXIT

dd if=/dev/zero of="$ESP_IMG" bs=1M count=64 2>/dev/null
mkfs.vfat "$ESP_IMG" >/dev/null 2>&1

MNT=$(mktemp -d)
sudo mount "$ESP_IMG" "$MNT"

# LamBoot itself
sudo mkdir -p "$MNT/EFI/BOOT"
sudo cp dist/EFI/LamBoot/lambootx64.efi "$MNT/EFI/BOOT/BOOTX64.EFI"
sudo mkdir -p "$MNT/EFI/LamBoot"
sudo cp dist/EFI/LamBoot/lambootx64.efi "$MNT/EFI/LamBoot/"
sudo cp dist/EFI/LamBoot/policy.toml "$MNT/EFI/LamBoot/"
sudo mkdir -p "$MNT/EFI/LamBoot/drivers" "$MNT/EFI/LamBoot/modules" "$MNT/EFI/LamBoot/reports"

# Deliberately NO BLS entries (no /loader/entries/ directory with .conf files)
# Deliberately NO UKIs (no /EFI/Linux/*.efi)
# Deliberately NO \EFI\Microsoft, no distro directories, no other loaders.
# LamBoot should hit the "no bootable entries" recovery screen.

sudo umount "$MNT"
rmdir "$MNT"

echo "=== Task #40 no-entries recovery test ==="
echo "ESP contains only LamBoot itself — no BLS entries, no UKI, no other loaders."
echo "Expected: LamBoot discovery finds zero entries, renders recovery screen with F2/F12/Power-off."
echo "Watch the serial output. Exit with Ctrl+A then X."
echo
echo "--- serial output follows ---"
echo

qemu-system-x86_64 \
    -machine q35 \
    -enable-kvm \
    -cpu host \
    -m 1024 \
    -drive if=pflash,format=raw,readonly=on,file=/usr/share/OVMF/OVMF_CODE_4M.fd \
    -drive if=pflash,format=raw,file="$TEMP_VARS" \
    -drive format=raw,file="$ESP_IMG" \
    -net none \
    -nographic \
    -serial mon:stdio
