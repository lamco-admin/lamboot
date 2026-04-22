#!/bin/bash
# build-ovmf-vars.sh — Generate OVMF VARS file with LamBoot's signing key in db.
#
# Creates a VARS file that includes Microsoft keys (for Windows/shim compatibility)
# plus LamBoot's signing certificate in the firmware db. VMs using this VARS file
# can load LamBoot directly under Secure Boot without MOK enrollment or shim.
#
# Usage:
#   ./tools/build-ovmf-vars.sh                           # Use default keys
#   ./tools/build-ovmf-vars.sh --cert keys/db.crt        # Use specific cert
#   ./tools/build-ovmf-vars.sh --output /path/to/output   # Custom output path
#
# Prerequisites:
#   pip install virt-firmware (or use a venv)
#   OVMF VARS template: /usr/share/OVMF/OVMF_VARS_4M.ms.fd

set -euo pipefail

# Defaults
CERT="${1:-keys/db.crt}"
OUTPUT="dist/OVMF_VARS_lamboot.fd"
VARS_TEMPLATE=""
VIRT_FW_VARS=""

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --cert) CERT="$2"; shift 2 ;;
        --output) OUTPUT="$2"; shift 2 ;;
        --template) VARS_TEMPLATE="$2"; shift 2 ;;
        *) shift ;;
    esac
done

# Find virt-fw-vars
for candidate in \
    virt-fw-vars \
    /tmp/virt-fw-venv/bin/virt-fw-vars \
    ~/.local/bin/virt-fw-vars; do
    if command -v "$candidate" >/dev/null 2>&1 || [ -x "$candidate" ]; then
        VIRT_FW_VARS="$candidate"
        break
    fi
done

[ -n "$VIRT_FW_VARS" ] || { echo "ERROR: virt-fw-vars not found. Install: pip install virt-firmware" >&2; exit 1; }

# Find OVMF VARS template (with Microsoft keys pre-enrolled)
if [ -z "$VARS_TEMPLATE" ]; then
    for template in \
        /usr/share/OVMF/OVMF_VARS_4M.ms.fd \
        /usr/share/qemu/OVMF_VARS_4M.ms.fd \
        /usr/share/edk2/ovmf/OVMF_VARS.ms.fd; do
        if [ -f "$template" ]; then
            VARS_TEMPLATE="$template"
            break
        fi
    done
fi

[ -n "$VARS_TEMPLATE" ] || { echo "ERROR: No OVMF VARS template found. Install: apt install ovmf" >&2; exit 1; }

# Verify certificate exists
if [ ! -f "$CERT" ]; then
    echo "ERROR: Certificate not found: $CERT"
    echo "Generate keys first: ./tools/sign-lamboot.sh"
    exit 1
fi

# Convert PEM to DER if needed
CERT_DER="$CERT"
if head -1 "$CERT" 2>/dev/null | grep -q "BEGIN CERTIFICATE"; then
    CERT_DER="${CERT%.crt}.der"
    openssl x509 -in "$CERT" -outform der -out "$CERT_DER"
fi

echo "Building OVMF VARS with LamBoot key..."
echo "  Template: $VARS_TEMPLATE"
echo "  Certificate: $CERT"
echo "  Output: $OUTPUT"
echo ""

# Start from the Microsoft-keyed template (has MS keys in PK, KEK, db)
cp "$VARS_TEMPLATE" "$OUTPUT"

# Add LamBoot's certificate to db
# GUID is arbitrary — identifies who enrolled this key
LAMBOOT_GUID="4c414d42-4f4f-5400-0000-000000000002"

"$VIRT_FW_VARS" --inplace "$OUTPUT" \
    --add-db "$LAMBOOT_GUID" "$CERT_DER"

echo ""
echo "=== OVMF VARS Built ==="
echo "Output: $OUTPUT ($(stat -c %s "$OUTPUT") bytes)"
echo ""
echo "To use with Proxmox:"
echo "  1. Copy to Proxmox host: scp $OUTPUT root@proxmox:/usr/share/kvm/"
echo "  2. Create VM with custom VARS:"
echo "     qm create VMID --efidisk0 local-lvm:1,efitype=4m,pre-enrolled-keys=0"
echo "     # Then replace the VARS file with our custom one"
echo ""
echo "To use with libvirt/QEMU:"
echo "  qemu-system-x86_64 \\"
echo "    -drive if=pflash,format=raw,readonly=on,file=/usr/share/OVMF/OVMF_CODE_4M.secboot.fd \\"
echo "    -drive if=pflash,format=raw,file=$OUTPUT"
