#!/bin/bash
# Sign LamBoot EFI binaries for Secure Boot.
#
# Prerequisites:
#   - keys/db.key and keys/db.crt must exist (see docs/KEY-GENERATION.md)
#   - sbsign, sbverify, objcopy, openssl installed
#
# Usage:
#   source tools/sign-unlock          # once per terminal session
#   ./tools/sign-lamboot.sh           # sign using unlocked key
#
# Or (prompts for passphrase every invocation):
#   ./tools/sign-lamboot.sh --interactive

set -e

BINARY="target/x86_64-unknown-uefi/release/lambootx64.efi"
SBAT_CSV="dist/sbat.csv"
DIST_DIR="dist/EFI/LamBoot"
DB_CERT="keys/db.crt"

# Use session-unlocked key if available, else fall back to the encrypted one
if [ -n "${LAMBOOT_SIGN_KEY:-}" ] && [ -f "$LAMBOOT_SIGN_KEY" ]; then
    DB_KEY="$LAMBOOT_SIGN_KEY"
else
    DB_KEY="keys/db.key"
    echo "Note: signing with encrypted keys/db.key (sbsign will prompt per binary)."
    echo "To avoid prompts for the rest of this session: source tools/sign-unlock"
fi

# Sanity checks
if [ ! -f "$DB_KEY" ]; then
    echo "ERROR: signing key not found at $DB_KEY" >&2
    echo "Generate production keys per docs/KEY-GENERATION.md first." >&2
    exit 1
fi

if [ ! -f "$DB_CERT" ]; then
    echo "ERROR: signing cert not found at $DB_CERT" >&2
    exit 1
fi

if [ ! -f "$BINARY" ]; then
    echo "ERROR: bootloader not built at $BINARY" >&2
    echo "Run cargo build or ./build.sh first." >&2
    exit 1
fi

# Stage binary and add SBAT section
WORK_BINARY="$BINARY"
if [ -f "$SBAT_CSV" ]; then
    WORK_BINARY="${BINARY%.efi}-sbat.efi"
    cp "$BINARY" "$WORK_BINARY"

    SBAT_SIZE=$(stat -c %s "$SBAT_CSV" 2>/dev/null || stat -f %z "$SBAT_CSV")
    PADDED_SIZE=$(( (SBAT_SIZE + 511) / 512 * 512 ))
    SBAT_PADDED=$(mktemp)
    cp "$SBAT_CSV" "$SBAT_PADDED"
    truncate -s "$PADDED_SIZE" "$SBAT_PADDED"

    # GNU objcopy corrupts UEFI PE binaries when adding sections.
    # llvm-objcopy handles PE correctly — use it if available, fall back to GNU.
    objcopy_bin=""
    if command -v llvm-objcopy >/dev/null 2>&1; then
        objcopy_bin=llvm-objcopy
    else
        echo "WARNING: llvm-objcopy not found, falling back to GNU objcopy." >&2
        echo "GNU objcopy is known to produce PE binaries that firmware rejects." >&2
        echo "Install LLVM tools for reliable signing: sudo apt install llvm" >&2
        objcopy_bin=objcopy
    fi
    "$objcopy_bin" --add-section .sbat="$SBAT_PADDED" \
            --set-section-flags .sbat=contents,alloc,load,readonly,data \
            "$WORK_BINARY"
    rm "$SBAT_PADDED"
    echo "Added .sbat section ($PADDED_SIZE bytes, via $objcopy_bin)"
fi

# Sign the main bootloader — output to BOTH target/ and dist/ for release packaging
TARGET_OUTPUT="${BINARY%.efi}-signed.efi"
DIST_OUTPUT="${DIST_DIR}/lambootx64-signed.efi"

sbsign --key "$DB_KEY" --cert "$DB_CERT" "$WORK_BINARY" --output "$TARGET_OUTPUT"
cp "$TARGET_OUTPUT" "$DIST_OUTPUT"
echo "Signed: $DIST_OUTPUT"

if [ "$WORK_BINARY" != "$BINARY" ]; then
    rm "$WORK_BINARY"
fi

# Sanity-check the signature against the cert that just signed it
if ! sbverify --cert "$DB_CERT" "$DIST_OUTPUT" >/dev/null 2>&1; then
    echo "ERROR: sbverify failed on $DIST_OUTPUT" >&2
    exit 1
fi
echo "  sbverify OK"

# Sign diagnostic modules
SIGNED_COUNT=0
MODULE_DIR="${DIST_DIR}/modules"
if [ -d "$MODULE_DIR" ]; then
    for mod_file in "$MODULE_DIR"/*.efi; do
        [ -f "$mod_file" ] || continue
        case "$mod_file" in *-signed.efi) continue;; esac

        signed_file="${mod_file%.efi}-signed.efi"
        sbsign --key "$DB_KEY" --cert "$DB_CERT" "$mod_file" --output "$signed_file"
        echo "  Signed module: $(basename "$mod_file")"
        SIGNED_COUNT=$((SIGNED_COUNT + 1))
    done
fi

# Sign filesystem drivers
DRIVER_DIR="${DIST_DIR}/drivers"
if [ -d "$DRIVER_DIR" ]; then
    for drv_file in "$DRIVER_DIR"/*.efi; do
        [ -f "$drv_file" ] || continue
        case "$drv_file" in *-signed.efi) continue;; esac

        signed_file="${drv_file%.efi}-signed.efi"
        sbsign --key "$DB_KEY" --cert "$DB_CERT" "$drv_file" --output "$signed_file"
        echo "  Signed driver: $(basename "$drv_file")"
        SIGNED_COUNT=$((SIGNED_COUNT + 1))
    done
fi

# Copy db.der into dist for MOK enrollment by the install script
if [ -f keys/db.der ]; then
    cp keys/db.der "$DIST_DIR/db.der"
    echo "  Published: $DIST_DIR/db.der (for MOK enrollment)"
fi

echo ""
echo "=== Signing Summary ==="
echo "  Bootloader: $DIST_OUTPUT ($(stat -c %s "$DIST_OUTPUT" 2>/dev/null || stat -f %z "$DIST_OUTPUT") bytes)"
echo "  Modules/drivers signed: $SIGNED_COUNT"
echo "  Signed with: $DB_CERT ($(openssl x509 -in "$DB_CERT" -noout -subject | sed 's/^subject=//'))"
echo ""
echo "For installation on an SB-enabled target:"
echo "  lamboot-install --signed"
