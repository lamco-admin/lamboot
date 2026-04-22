#!/bin/bash
# Diagnostic 3: sign the binary WITHOUT the objcopy SBAT step, then boot
# with SB DISABLED. If THIS works, the SBAT-adding objcopy step is breaking
# the PE. If it still fails, sbsign itself is the bug.

set -e

export PATH="/usr/sbin:/sbin:$PATH"

# Key must be unlocked in the calling shell
if [ -z "${LAMBOOT_SIGN_KEY:-}" ] || [ ! -f "${LAMBOOT_SIGN_KEY}" ]; then
    echo "ERROR: session key not unlocked. Run first:"
    echo "  source tools/sign-unlock"
    exit 1
fi

echo "=== Step 1: sign the binary with sbsign ONLY (no objcopy SBAT) ==="
sbsign --key "$LAMBOOT_SIGN_KEY" --cert keys/db.crt \
    --output dist/EFI/LamBoot/lambootx64-signed-nosbat.efi \
    dist/EFI/LamBoot/lambootx64.efi
sbverify --cert keys/db.crt dist/EFI/LamBoot/lambootx64-signed-nosbat.efi
echo

echo "=== Step 2: build ESP with the no-SBAT signed binary ==="
ESP_IMG="target/esp-nosbat.img"
rm -f "$ESP_IMG"
dd if=/dev/zero of="$ESP_IMG" bs=1M count=64 status=none
mkfs.vfat -n LAMBOOT "$ESP_IMG" >/dev/null
mmd -i "$ESP_IMG" ::/EFI
mmd -i "$ESP_IMG" ::/EFI/BOOT
mmd -i "$ESP_IMG" ::/EFI/LamBoot
mcopy -i "$ESP_IMG" dist/EFI/LamBoot/lambootx64-signed-nosbat.efi ::/EFI/BOOT/BOOTX64.EFI
mcopy -i "$ESP_IMG" dist/EFI/LamBoot/lambootx64-signed-nosbat.efi ::/EFI/LamBoot/lambootx64.efi
echo "ESP built."
echo

echo "=== Step 3: boot with SB disabled ==="
echo "If LamBoot splash appears: SBAT objcopy step is the bug (fixable)."
echo "If same BdsDxe Load Error: sbsign itself is producing a bad binary."
echo "Ctrl+A then X to exit."
echo "--- serial output follows ---"
echo

TEMP_VARS=$(mktemp --suffix=.fd)
cp /usr/share/OVMF/OVMF_VARS_4M.fd "$TEMP_VARS"
trap "rm -f $TEMP_VARS" EXIT

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
