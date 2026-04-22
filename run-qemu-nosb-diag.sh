#!/bin/bash
# Diagnostic 1: boot the SAME signed binary on the SAME ESP with Secure Boot
# DISABLED. Serial-only — no window, safe for remote display.
#
# Press Ctrl+A then X to exit QEMU.

set -e

ESP_IMG="target/esp-secureboot.img"

if [ ! -f "$ESP_IMG" ]; then
    echo "ERROR: $ESP_IMG not found. Run ./run-qemu-secureboot.sh first to build it."
    exit 1
fi

# Working writable copy of VARS (system file is read-only)
TEMP_VARS=$(mktemp --suffix=.fd)
cp /usr/share/OVMF/OVMF_VARS_4M.fd "$TEMP_VARS"
trap "rm -f $TEMP_VARS" EXIT

echo "=== Secure Boot DISABLED diagnostic ==="
echo "Watch the serial output below. Exit with Ctrl+A then X."
echo "If you see LamBoot startup logs, the binary is fine and the SB failure"
echo "is in cert chaining. If you see the same BdsDxe Load Error, the bug is"
echo "in the binary/ESP, not SB."
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
