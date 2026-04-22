#!/bin/bash
# Run LamBoot in QEMU with Secure Boot ENABLED and LamBoot's production key
# pre-enrolled in firmware db (Config 4 path validation).
#
# This is the end-to-end smoke test: SB-enforcing firmware attempts to verify
# the signed lambootx64.efi against the enrolled cert in db. If the signed
# binary boots, the full signing pipeline is validated.
#
# Prerequisites:
#   - OVMF package with OVMF_CODE_4M.secboot.fd (apt install ovmf)
#   - dist/EFI/LamBoot/lambootx64-signed.efi (run: ./build.sh && ./tools/sign-lamboot.sh)
#   - dist/OVMF_VARS_lamboot.fd (rebuild with: ./tools/build-ovmf-vars.sh)

set -e

# /sbin holds mkfs.vfat on Debian 13 and is not always in user PATH
export PATH="/usr/sbin:/sbin:$PATH"

OVMF_CODE="/usr/share/OVMF/OVMF_CODE_4M.secboot.fd"
OVMF_VARS_SRC="dist/OVMF_VARS_lamboot.fd"
SIGNED_BINARY="dist/EFI/LamBoot/lambootx64-signed.efi"

for f in "$OVMF_CODE" "$OVMF_VARS_SRC" "$SIGNED_BINARY"; do
    if [ ! -f "$f" ]; then
        echo "ERROR: missing required file: $f" >&2
        case "$f" in
            *secboot.fd)    echo "  Install: sudo apt install ovmf" >&2 ;;
            *VARS_lamboot*) echo "  Build:   ./tools/build-ovmf-vars.sh" >&2 ;;
            *signed.efi)    echo "  Build:   ./build.sh && ./tools/sign-lamboot.sh" >&2 ;;
        esac
        exit 1
    fi
done

# Working copy of VARS (don't mutate the release artifact)
TEMP_VARS=$(mktemp --suffix=.fd)
cp "$OVMF_VARS_SRC" "$TEMP_VARS"

# Build an ESP image with the signed binary installed as BOOTX64.EFI plus
# the full dist/ tree available under \EFI\LamBoot\ (drivers, modules, etc.)
ESP_IMG="target/esp-secureboot.img"
mkdir -p target
dd if=/dev/zero of="$ESP_IMG" bs=1M count=64 status=none
mkfs.vfat -n LAMBOOT "$ESP_IMG" >/dev/null

# mtools avoids needing sudo for loop mount
mmd -i "$ESP_IMG" ::/EFI
mmd -i "$ESP_IMG" ::/EFI/BOOT
mmd -i "$ESP_IMG" ::/EFI/LamBoot
mmd -i "$ESP_IMG" ::/EFI/LamBoot/drivers
mmd -i "$ESP_IMG" ::/EFI/LamBoot/modules

# BOOTX64.EFI is the signed binary (firmware fallback path)
mcopy -i "$ESP_IMG" "$SIGNED_BINARY" ::/EFI/BOOT/BOOTX64.EFI
mcopy -i "$ESP_IMG" "$SIGNED_BINARY" "::/EFI/LamBoot/lambootx64.efi"

# Signed drivers + modules — under SB these MUST be signed variants
for drv in dist/EFI/LamBoot/drivers/*-signed.efi; do
    [ -f "$drv" ] || continue
    # Remove the -signed suffix for the on-disk name LamBoot expects
    name=$(basename "$drv" | sed 's/-signed//')
    mcopy -i "$ESP_IMG" "$drv" "::/EFI/LamBoot/drivers/${name}"
done
for mod in dist/EFI/LamBoot/modules/*-signed.efi; do
    [ -f "$mod" ] || continue
    name=$(basename "$mod" | sed 's/-signed//')
    mcopy -i "$ESP_IMG" "$mod" "::/EFI/LamBoot/modules/${name}"
done

# Configs (unsigned, not validated by SB)
[ -f dist/EFI/LamBoot/policy.toml ] && mcopy -i "$ESP_IMG" dist/EFI/LamBoot/policy.toml ::/EFI/LamBoot/policy.toml
[ -f dist/EFI/LamBoot/modules/manifest.toml ] && mcopy -i "$ESP_IMG" dist/EFI/LamBoot/modules/manifest.toml ::/EFI/LamBoot/modules/manifest.toml

echo "=== ESP contents ==="
mdir -i "$ESP_IMG" ::/EFI/BOOT
mdir -i "$ESP_IMG" ::/EFI/LamBoot
echo
echo "=== Launching QEMU with Secure Boot ENABLED ==="
echo "  CODE: $OVMF_CODE"
echo "  VARS: $TEMP_VARS (production db cert pre-enrolled)"
echo "  ESP:  $ESP_IMG"
echo
echo "Expected: LamBoot splash appears. If you see a red 'Secure Boot Violation'"
echo "or the firmware falls through to a shell/bsod, the signed binary did not"
echo "verify against the enrolled cert — the signing pipeline has a bug."
echo
echo "Press Ctrl+A then X in the terminal to exit QEMU."
echo

qemu-system-x86_64 \
    -machine q35,smm=on \
    -global driver=cfi.pflash01,property=secure,value=on \
    -enable-kvm \
    -cpu host \
    -m 1024 \
    -drive if=pflash,format=raw,unit=0,readonly=on,file="$OVMF_CODE" \
    -drive if=pflash,format=raw,unit=1,file="$TEMP_VARS" \
    -drive format=raw,file="$ESP_IMG" \
    -net none \
    -vga std \
    -serial stdio \
    -display gtk

echo
echo "QEMU exited. Cleaning up working VARS copy."
rm -f "$TEMP_VARS"
