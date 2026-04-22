#!/bin/bash
# Diagnostic 2: rebuild ESP with the UNSIGNED binary (no objcopy, no sbsign)
# and boot with SB DISABLED. If THIS works, objcopy or sbsign is corrupting
# the PE. If this ALSO fails, the bug is in the Rust build output itself.

set -e

export PATH="/usr/sbin:/sbin:$PATH"

ESP_IMG="target/esp-unsigned.img"

# Build fresh ESP with the UNSIGNED binary
rm -f "$ESP_IMG"
dd if=/dev/zero of="$ESP_IMG" bs=1M count=64 status=none
mkfs.vfat -n LAMBOOT "$ESP_IMG" >/dev/null
mmd -i "$ESP_IMG" ::/EFI
mmd -i "$ESP_IMG" ::/EFI/BOOT
mmd -i "$ESP_IMG" ::/EFI/LamBoot
mcopy -i "$ESP_IMG" dist/EFI/LamBoot/lambootx64.efi ::/EFI/BOOT/BOOTX64.EFI
mcopy -i "$ESP_IMG" dist/EFI/LamBoot/lambootx64.efi ::/EFI/LamBoot/lambootx64.efi
echo "ESP built with UNSIGNED binary."
echo

# Writable VARS copy
TEMP_VARS=$(mktemp --suffix=.fd)
cp /usr/share/OVMF/OVMF_VARS_4M.fd "$TEMP_VARS"
trap "rm -f $TEMP_VARS" EXIT

echo "=== UNSIGNED binary, SB DISABLED ==="
echo "Ctrl+A then X to exit."
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
