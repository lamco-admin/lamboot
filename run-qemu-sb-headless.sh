#!/bin/bash
# Headless end-to-end SB test. Runs in the background with a timeout, writes
# serial output to a file, prints results at the end. Does NOT steal the
# terminal or require interactive exit.

set -e

export PATH="/usr/sbin:/sbin:$PATH"

if [ -z "${LAMBOOT_SIGN_KEY:-}" ] || [ ! -f "${LAMBOOT_SIGN_KEY}" ]; then
    echo "ERROR: session key not unlocked. Run first:"
    echo "  source tools/sign-unlock"
    exit 1
fi

# Clean prior artifacts so we see fresh behavior
rm -f dist/EFI/LamBoot/lambootx64-signed.efi
rm -f target/x86_64-unknown-uefi/release/lambootx64-signed.efi
rm -f target/x86_64-unknown-uefi/release/lambootx64-sbat.efi

echo "=== Re-signing with fixed pipeline (llvm-objcopy + sbsign) ==="
./tools/sign-lamboot.sh | tail -10
echo

echo "=== Verifying signature chains to production cert ==="
sbverify --cert keys/db.crt dist/EFI/LamBoot/lambootx64-signed.efi
echo

echo "=== Building ESP ==="
ESP_IMG="target/esp-sb-headless.img"
rm -f "$ESP_IMG"
dd if=/dev/zero of="$ESP_IMG" bs=1M count=64 status=none
mkfs.vfat -n LAMBOOT "$ESP_IMG" >/dev/null
mmd -i "$ESP_IMG" ::/EFI ::/EFI/BOOT ::/EFI/LamBoot
mcopy -i "$ESP_IMG" dist/EFI/LamBoot/lambootx64-signed.efi ::/EFI/BOOT/BOOTX64.EFI
mcopy -i "$ESP_IMG" dist/EFI/LamBoot/lambootx64-signed.efi ::/EFI/LamBoot/lambootx64.efi
echo "  done"
echo

echo "=== Booting under Secure Boot ENFORCING firmware ==="
echo "Firmware: OVMF_CODE_4M.secboot.fd"
echo "VARS:     dist/OVMF_VARS_lamboot.fd (production LamBoot cert in db)"
echo "Running headless for 20 seconds..."
echo

TEMP_VARS=$(mktemp --suffix=.fd)
cp dist/OVMF_VARS_lamboot.fd "$TEMP_VARS"
SERIAL_LOG=$(mktemp --suffix=.log)
QEMU_ERR=$(mktemp --suffix=.log)

setsid timeout 20 qemu-system-x86_64 \
    -machine q35,smm=on \
    -global driver=cfi.pflash01,property=secure,value=on \
    -m 512 \
    -drive if=pflash,format=raw,readonly=on,file=/usr/share/OVMF/OVMF_CODE_4M.secboot.fd \
    -drive if=pflash,format=raw,file="$TEMP_VARS" \
    -drive format=raw,file="$ESP_IMG" \
    -net none \
    -display none \
    -vga none \
    -serial file:"$SERIAL_LOG" \
    -monitor none \
    </dev/null >/dev/null 2>"$QEMU_ERR" &
QPID=$!
# Wait for qemu to finish (timeout will kill it after 20s)
wait "$QPID" 2>/dev/null || true

echo "=== Serial output ==="
cat "$SERIAL_LOG" 2>/dev/null || echo "(empty)"
echo
echo "=== QEMU stderr ==="
cat "$QEMU_ERR" 2>/dev/null | head -5
echo
echo "=== Verdict ==="
if grep -q 'LamBoot v' "$SERIAL_LOG" 2>/dev/null; then
    echo "SUCCESS: LamBoot loaded under Secure Boot. Pipeline validated."
elif grep -q 'Load Error' "$SERIAL_LOG" 2>/dev/null; then
    echo "FAIL: firmware rejected the signed binary. Signature chain issue."
    echo "      (Cert may not be properly enrolled in VARS db.)"
else
    echo "INCONCLUSIVE: no clear LamBoot start or Load Error in log."
fi

rm -f "$TEMP_VARS" "$SERIAL_LOG" "$QEMU_ERR"
