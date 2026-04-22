#!/bin/bash
# Diagnostic 4: rebuild the signed-with-SBAT binary using the fixed
# sign-lamboot.sh (now uses llvm-objcopy instead of GNU objcopy), then test.

set -e

export PATH="/usr/sbin:/sbin:$PATH"

if [ -z "${LAMBOOT_SIGN_KEY:-}" ] || [ ! -f "${LAMBOOT_SIGN_KEY}" ]; then
    echo "ERROR: session key not unlocked. Run first:"
    echo "  source tools/sign-unlock"
    exit 1
fi

# Clean old signed artifacts
rm -f dist/EFI/LamBoot/lambootx64-signed.efi
rm -f target/x86_64-unknown-uefi/release/lambootx64-signed.efi
rm -f target/x86_64-unknown-uefi/release/lambootx64-sbat.efi

echo "=== Step 1: re-sign via fixed sign-lamboot.sh (llvm-objcopy for SBAT) ==="
./tools/sign-lamboot.sh
echo

echo "=== Step 2: rebuild ESP with new signed binary ==="
ESP_IMG="target/esp-llvm.img"
rm -f "$ESP_IMG"
dd if=/dev/zero of="$ESP_IMG" bs=1M count=64 status=none
mkfs.vfat -n LAMBOOT "$ESP_IMG" >/dev/null
mmd -i "$ESP_IMG" ::/EFI
mmd -i "$ESP_IMG" ::/EFI/BOOT
mmd -i "$ESP_IMG" ::/EFI/LamBoot
mcopy -i "$ESP_IMG" dist/EFI/LamBoot/lambootx64-signed.efi ::/EFI/BOOT/BOOTX64.EFI
mcopy -i "$ESP_IMG" dist/EFI/LamBoot/lambootx64-signed.efi ::/EFI/LamBoot/lambootx64.efi
echo "ESP built."
echo

echo "=== Step 3: boot with SB DISABLED to verify binary is structurally valid ==="
echo "If LamBoot loads: the llvm-objcopy fix works, pipeline is good."
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
