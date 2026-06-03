#!/usr/bin/env bats
#
# Dual contract for the update / upgrade / remove machinery, at the unit level
# (the functions that decide what gets rewritten, what NVRAM is committed, and
# how rollback state is captured). Full main() integration across the whole
# pipeline is out of scope for the host suite (it needs a live UEFI/efivars
# view); these tests pin the load-bearing decision functions instead.
#
# flexibility: idempotent same-version --update must not rewrite identical files;
#   an upgrade (different bytes) must replace and re-stamp the manifest version;
#   a clean install must verify and commit.
# security: NVRAM must never be pointed at a missing/mismatched loader or a
#   half-built shim chain; the rollback snapshot must never record LamBoot's own
#   bootnum; a tampered manifest must not inject shell or extra delete targets.

load helper

setup() {
    setup_mock_path
    mock_stdout_var efibootmgr MOCK_EBM
    load_installer
    set +u
    ARCH="x86_64"
    ESP="${BATS_TEST_TMPDIR}/esp"
    mkdir -p "${ESP}/${EFI_DIR}"
    MANIFEST_ENTRIES=()
    OPT_DRY_RUN=0
    SECURE_BOOT=0
    SHIM_SOURCE=""
    OPT_NO_SHIM=0
    PRIOR_BOOTORDER_CAPTURED=0
}

# ===========================================================================
# needs_update — hash-driven replace decision
# ===========================================================================

@test "flex: needs_update is true when the destination is absent (fresh/new file)" {
    printf 'AAAA' > "${BATS_TEST_TMPDIR}/src"
    lb needs_update "${BATS_TEST_TMPDIR}/src" "${BATS_TEST_TMPDIR}/nope"
    [ "$status" -eq 0 ]
}

@test "flex: needs_update is false when src and dst are byte-identical (idempotent)" {
    printf 'AAAA' > "${BATS_TEST_TMPDIR}/src"
    printf 'AAAA' > "${BATS_TEST_TMPDIR}/dst"
    lb needs_update "${BATS_TEST_TMPDIR}/src" "${BATS_TEST_TMPDIR}/dst"
    [ "$status" -ne 0 ]
}

@test "flex: needs_update is true when bytes differ (upgrade replaces)" {
    printf 'NEWVERSION' > "${BATS_TEST_TMPDIR}/src"
    printf 'OLDVERSION' > "${BATS_TEST_TMPDIR}/dst"
    lb needs_update "${BATS_TEST_TMPDIR}/src" "${BATS_TEST_TMPDIR}/dst"
    [ "$status" -eq 0 ]
}

@test "sec: an unverifiable dst (empty hash) is treated as needs-copy, never skipped" {
    printf 'AAAA' > "${BATS_TEST_TMPDIR}/src"
    printf 'AAAA' > "${BATS_TEST_TMPDIR}/dst"
    # Simulate file_sha256 failing to read the destination (returns empty).
    file_sha256() { case "$1" in *dst) echo "";; *) command sha256sum -- "$1" | cut -d' ' -f1;; esac; }
    lb needs_update "${BATS_TEST_TMPDIR}/src" "${BATS_TEST_TMPDIR}/dst"
    [ "$status" -eq 0 ]
}

# ===========================================================================
# write_manifest / read_manifest — round-trip + version stamp + tamper safety
# ===========================================================================

@test "flex: write_manifest stamps the current version and round-trips through read_manifest" {
    printf 'loaderbytes' > "${ESP}/${EFI_DIR}/lambootx64.efi"
    manifest_add "${EFI_DIR}/lambootx64.efi"
    DISTRO_ID="ubuntu"
    lbq write_manifest
    [ "$status" -eq 0 ]
    [ -f "${ESP}/${MANIFEST_PATH}" ]
    grep -q "# Version: ${LAMBOOT_VERSION}" "${ESP}/${MANIFEST_PATH}"
    MANIFEST_HASHES=()
    lbq read_manifest
    [ "$status" -eq 0 ]
    [ "$MANIFEST_VERSION" = "${LAMBOOT_VERSION}" ]
    [ -n "${MANIFEST_HASHES[${EFI_DIR}/lambootx64.efi]}" ]
}

@test "flex: --dry-run write_manifest leaves the manifest untouched" {
    OPT_DRY_RUN=1
    MANIFEST_ENTRIES=("sha256:deadbeef  ${EFI_DIR}/lambootx64.efi")
    write_manifest
    [ ! -f "${ESP}/${MANIFEST_PATH}" ]
}

@test "sec: read_manifest ignores junk/non-sha256 lines and never executes them" {
    {
        echo "# LamBoot Install Manifest"
        echo "# Version: 0.14.0"
        echo "rm -rf /"
        echo "notasha:xyz  ${EFI_DIR}/evil.efi"
        echo "sha256:abc123  ${EFI_DIR}/lambootx64.efi"
    } > "${ESP}/${MANIFEST_PATH}"
    MANIFEST_HASHES=()
    lbq read_manifest
    [ "$status" -eq 0 ]
    [ "${MANIFEST_HASHES[${EFI_DIR}/lambootx64.efi]}" = "abc123" ]
    [ -z "${MANIFEST_HASHES[${EFI_DIR}/evil.efi]:-}" ]
}

@test "flex: read_manifest returns non-zero when the manifest is absent" {
    lb read_manifest
    [ "$status" -ne 0 ]
}

# ===========================================================================
# verify_loader_present_for_nvram — two-phase-commit gate before NVRAM write
# ===========================================================================

@test "flex: complete unsigned install passes the NVRAM gate" {
    printf 'loaderbytes' > "${ESP}/${EFI_DIR}/lambootx64.efi"
    manifest_add "${EFI_DIR}/lambootx64.efi"
    SECURE_BOOT=0
    lb verify_loader_present_for_nvram
    [ "$status" -eq 0 ]
}

@test "sec: NVRAM gate blocks when the loader copy is missing (failed phase-4)" {
    # No lambootx64.efi written; manifest claims it.
    MANIFEST_ENTRIES=("sha256:abc  ${EFI_DIR}/lambootx64.efi")
    SECURE_BOOT=0
    lb verify_loader_present_for_nvram
    [ "$status" -ne 0 ]
}

@test "sec: NVRAM gate blocks on on-disk/manifest hash mismatch" {
    printf 'loaderbytes' > "${ESP}/${EFI_DIR}/lambootx64.efi"
    # Manifest records a hash that does NOT match the on-disk bytes.
    MANIFEST_ENTRIES=("sha256:0000000000  ${EFI_DIR}/lambootx64.efi")
    SECURE_BOOT=0
    lb verify_loader_present_for_nvram
    [ "$status" -ne 0 ]
}

@test "sec: under SB+shim the gate requires the full shim->grub chain present" {
    printf 'loaderbytes' > "${ESP}/${EFI_DIR}/lambootx64.efi"
    printf 'shimbytes'   > "${ESP}/${EFI_DIR}/shimx64.efi"
    manifest_add "${EFI_DIR}/lambootx64.efi"
    manifest_add "${EFI_DIR}/shimx64.efi"
    # grubx64.efi (the shim chain target) is deliberately absent.
    SECURE_BOOT=1
    SHIM_SOURCE="/boot/efi/EFI/ubuntu/shimx64.efi"
    OPT_NO_SHIM=0
    lb verify_loader_present_for_nvram
    [ "$status" -ne 0 ]
}

# ===========================================================================
# capture_prior_bootorder — rollback snapshot
# ===========================================================================

@test "flex: first install records prior BootOrder and strips LamBoot's own bootnum" {
    export MOCK_EBM=$'BootCurrent: 0001\nBootOrder: 0001,0000\nBoot0001* LamBoot\nBoot0000* ubuntu'
    capture_prior_bootorder
    [ -f "${ESP}/${EFI_DIR}/.bootorder-backup" ]
    [ "$(cat "${ESP}/${EFI_DIR}/.bootorder-backup")" = "0000" ]
}

@test "sec: an existing rollback marker is preserved, never overwritten on re-run" {
    printf '0000,0002\n' > "${ESP}/${EFI_DIR}/.bootorder-backup"
    export MOCK_EBM=$'BootOrder: 0001,0000,0002\nBoot0001* LamBoot'
    capture_prior_bootorder
    [ "$(cat "${ESP}/${EFI_DIR}/.bootorder-backup")" = "0000,0002" ]
}

@test "flex: --dry-run capture writes no marker" {
    OPT_DRY_RUN=1
    export MOCK_EBM=$'BootOrder: 0000\nBoot0000* ubuntu'
    capture_prior_bootorder
    [ ! -f "${ESP}/${EFI_DIR}/.bootorder-backup" ]
}
