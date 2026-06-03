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

# Per-arch source binaries — both are signed in a single invocation
# so a release ships consistent signed artifacts for every platform
# the PKGBUILD declares (arch=('x86_64' 'aarch64')). v0.11.7 and prior
# only signed x86_64, which left lambootaa64.efi unsigned in every
# release tarball — invisible breakage for any aarch64 Secure Boot
# user. Track 1 (lambootaa64 signing) fixed in v0.11.8.
# Note: the cargo bin is named `lambootx64` in lamboot-core's Cargo.toml,
# so the aarch64 build artifact lands at `lambootx64.efi` under the
# aarch64-unknown-uefi target dir (not `lambootaa64.efi`). build.sh
# renames at copy-to-dist time. Worth fixing the bin name post-v0.11.8
# but out-of-scope for this release.
declare -A ARCH_BINARIES=(
    [x86_64]="target/x86_64-unknown-uefi/release/lambootx64.efi"
    [aarch64]="target/aarch64-unknown-uefi/release/lambootx64.efi"
)
declare -A ARCH_DIST_NAMES=(
    [x86_64]="lambootx64-signed.efi"
    [aarch64]="lambootaa64-signed.efi"
)
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

# Resolve which objcopy to use for SBAT-section addition. GNU objcopy
# corrupts UEFI PE binaries when adding sections; llvm-objcopy handles
# PE correctly. Resolved once and reused per-arch.
#
# Resolved ONLY when there is an SBAT section to embed (dist/sbat.csv present).
# objcopy is needed for nothing else, so a build with no SBAT csv must sign fine
# on a host without LLVM tools rather than hard-error on a tool it won't use.
#
# Missing llvm-objcopy is a HARD ERROR (when SBAT IS being embedded): the GNU
# fallback can produce a signed, sbverify-passing binary that firmware rejects
# (brick-on-deploy). Set LAMBOOT_ALLOW_GNU_OBJCOPY=1 to opt into the GNU path
# knowingly (e.g. a throwaway dev build you will not deploy).
objcopy_bin=""
if [ -f "$SBAT_CSV" ]; then
    if command -v llvm-objcopy >/dev/null 2>&1; then
        objcopy_bin=llvm-objcopy
    elif [ "${LAMBOOT_ALLOW_GNU_OBJCOPY:-0}" = "1" ]; then
        echo "WARNING: llvm-objcopy not found; using GNU objcopy (LAMBOOT_ALLOW_GNU_OBJCOPY=1)." >&2
        echo "GNU objcopy is known to produce PE binaries that firmware rejects — do NOT deploy." >&2
        objcopy_bin=objcopy
    else
        echo "ERROR: llvm-objcopy not found. GNU objcopy corrupts UEFI PE binaries when" >&2
        echo "       adding the SBAT section, producing a signed binary that firmware rejects." >&2
        echo "       Install LLVM tools (sudo apt install llvm), or set" >&2
        echo "       LAMBOOT_ALLOW_GNU_OBJCOPY=1 for a dev build you will not deploy." >&2
        exit 1
    fi
fi

# Assert a non-empty .sbat section exists in a signed PE, parsing the PE
# section table directly (GNU objdump misreads aarch64 PE). Exit 0 = present.
assert_sbat_section() {
    python3 - "$1" <<'PYEOF'
import struct, sys
d = open(sys.argv[1], "rb").read()
pe = struct.unpack("<I", d[0x3c:0x40])[0]
if d[pe:pe+4] != b"PE\0\0":
    sys.exit(1)
coff = pe + 4
nsec = struct.unpack("<H", d[coff+2:coff+4])[0]
optsz = struct.unpack("<H", d[coff+16:coff+18])[0]
sectab = coff + 20 + optsz
for i in range(nsec):
    off = sectab + i*40
    name = d[off:off+8].rstrip(b"\0")
    rawsize, rawptr = struct.unpack("<II", d[off+16:off+24])
    if name == b".sbat" and rawsize > 0 and d[rawptr:rawptr+rawsize].strip(b"\0"):
        sys.exit(0)
sys.exit(1)
PYEOF
}

SIGNED_BOOTLOADERS=()
for arch in x86_64 aarch64; do
    BINARY="${ARCH_BINARIES[$arch]}"
    DIST_OUTPUT="${DIST_DIR}/${ARCH_DIST_NAMES[$arch]}"

    if [ ! -f "$BINARY" ]; then
        # An arch-specific binary may be absent on dev hosts that only
        # built one target. Warn and skip rather than abort — the
        # release pipeline runs both builds and will surface a missing
        # target as a build error before reaching sign.
        echo "WARNING: $arch bootloader not built at $BINARY — skipping." >&2
        continue
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

        "$objcopy_bin" --add-section .sbat="$SBAT_PADDED" \
                --set-section-flags .sbat=contents,alloc,load,readonly,data \
                "$WORK_BINARY"
        rm "$SBAT_PADDED"
        echo "[$arch] Added .sbat section ($PADDED_SIZE bytes, via $objcopy_bin)"
    fi

    # Sign — output to BOTH target/ and dist/ for release packaging
    TARGET_OUTPUT="${BINARY%.efi}-signed.efi"

    sbsign --key "$DB_KEY" --cert "$DB_CERT" "$WORK_BINARY" --output "$TARGET_OUTPUT"
    cp "$TARGET_OUTPUT" "$DIST_OUTPUT"
    echo "[$arch] Signed: $DIST_OUTPUT"

    if [ "$WORK_BINARY" != "$BINARY" ]; then
        rm "$WORK_BINARY"
    fi

    if ! sbverify --cert "$DB_CERT" "$DIST_OUTPUT" >/dev/null 2>&1; then
        echo "ERROR: sbverify failed on $DIST_OUTPUT" >&2
        exit 1
    fi
    echo "[$arch]   sbverify OK"

    # Assert the .sbat section is actually present + non-empty. sbverify only
    # checks the Authenticode signature, not section presence — a future tool
    # that strips .sbat post-sign (the v0.11.8 failure mode) would still pass
    # sbverify while shipping an un-revocable binary. We parse the PE section
    # table directly rather than via `objdump -h`, which misreads aarch64 PE
    # (it reports a phantom .sbat with no extractable contents).
    if [ -f "$SBAT_CSV" ]; then
        if ! assert_sbat_section "$DIST_OUTPUT"; then
            echo "ERROR: .sbat section missing or empty in $DIST_OUTPUT" >&2
            echo "       (expected because $SBAT_CSV exists; the embed step must have failed)" >&2
            exit 1
        fi
        echo "[$arch]   .sbat section present"
    fi
    SIGNED_BOOTLOADERS+=("$DIST_OUTPUT")
done

if [ ${#SIGNED_BOOTLOADERS[@]} -eq 0 ]; then
    echo "ERROR: no bootloader binaries were signed (no target built)." >&2
    exit 1
fi

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
for b in "${SIGNED_BOOTLOADERS[@]}"; do
    sz=$(stat -c %s "$b" 2>/dev/null || stat -f %z "$b")
    echo "  Bootloader: $b ($sz bytes)"
done
echo "  Modules/drivers signed: $SIGNED_COUNT"
echo "  Signed with: $DB_CERT ($(openssl x509 -in "$DB_CERT" -noout -subject | sed 's/^subject=//'))"
echo ""
echo "For installation on an SB-enabled target:"
echo "  lamboot-install --signed"
