#!/usr/bin/env bats
#
# Dual contract for ESP classification + the validate_esp gate.
#
# is_esp_partition must ACCEPT every real ESP across the support matrix — the
# GPT ESP type GUID in any case, AND the MBR/msdos EFI System byte 0xef/0xEF
# that removable USB media and prebuilt appliance images report (flexibility;
# the security review narrowed this to GPT-GUID-only and broke those media) —
# while REJECTING vfat partitions that are not ESPs (Microsoft Basic Data,
# Linux 0x83, empty type) so a bootloader is never written to a data partition
# (security).
#
# validate_esp must enforce the authorization split the review restored: generic
# --force does NOT bypass the partition-type assertion (only emits a hint toward
# the dedicated flag); --force-foreign-esp is the ONLY type-gate override; and
# under --root (chroot/offline image) the host-view checks are skipped entirely,
# but --root / behaves like the live host.

load helper

setup() {
    setup_mock_path
    # is_esp_partition resolves the backing device then reads its PARTTYPE.
    mock_stdout_var findmnt MOCK_SOURCE      # SOURCE device for the mountpoint
    mock_stdout_var lsblk   MOCK_PARTTYPE    # PARTTYPE for that device
    export MOCK_SOURCE="/dev/sda1"
    load_installer
}

# ===========================================================================
# is_esp_partition — type classification
# ===========================================================================

@test "flex: canonical GPT ESP GUID (lowercase) accepted" {
    export MOCK_PARTTYPE="c12a7328-f81f-11d2-ba4b-00a0c93ec93b"
    lb is_esp_partition /boot/efi
    [ "$status" -eq 0 ]
}

@test "flex: GPT ESP GUID uppercase accepted (-i is load-bearing)" {
    export MOCK_PARTTYPE="C12A7328-F81F-11D2-BA4B-00A0C93EC93B"
    lb is_esp_partition /boot/efi
    [ "$status" -eq 0 ]
}

@test "flex: GPT ESP GUID mixed-case accepted" {
    export MOCK_PARTTYPE="C12a7328-F81f-11D2-Ba4b-00A0c93Ec93B"
    lb is_esp_partition /boot/efi
    [ "$status" -eq 0 ]
}

@test "flex: MBR EFI System byte 0xef accepted (USB / appliance media)" {
    export MOCK_PARTTYPE="0xef"
    lb is_esp_partition /boot/efi
    [ "$status" -eq 0 ]
}

@test "flex: MBR EFI System byte 0xEF (uppercase) accepted" {
    export MOCK_PARTTYPE="0xEF"
    lb is_esp_partition /boot/efi
    [ "$status" -eq 0 ]
}

@test "sec: 0xef00 (byte 0xef plus extra) rejected — '^0xef\$' is anchored" {
    export MOCK_PARTTYPE="0xef00"
    lb is_esp_partition /boot/efi
    [ "$status" -ne 0 ]
}

@test "sec: Linux MBR type 0x83 (vfat-on-data) rejected" {
    export MOCK_PARTTYPE="0x83"
    lb is_esp_partition /boot/efi
    [ "$status" -ne 0 ]
}

@test "sec: Microsoft Basic Data GUID (dual-boot vfat) rejected" {
    export MOCK_PARTTYPE="ebd0a0a2-b9e5-4433-87c0-68b6b72699c7"
    lb is_esp_partition /boot/efi
    [ "$status" -ne 0 ]
}

@test "sec: empty PARTTYPE (raw/whole-disk vfat, loop device) rejected" {
    export MOCK_PARTTYPE=""
    lb is_esp_partition /boot/efi
    [ "$status" -ne 0 ]
}

@test "sec: unresolvable mountpoint (no SOURCE device) rejected before lsblk" {
    export MOCK_SOURCE=""
    export MOCK_PARTTYPE="c12a7328-f81f-11d2-ba4b-00a0c93ec93b"
    lb is_esp_partition /boot/efi
    [ "$status" -ne 0 ]
}

# ===========================================================================
# validate_esp — gating logic
#
# Drive the leaf checks with controlled doubles so the branching is exercised
# directly: mountpoint/df are mocked by exit-code/stdout; is_vfat and
# is_esp_partition are overridden to read $VFAT_RC / $ESP_RC at call time. The
# real is_esp_partition is covered by the cases above.
# ===========================================================================

vesetup() {
    mock_rc mountpoint MOCK_MP_RC
    mock_stdout_var df MOCK_DF
    # Defaults: live host, no force, dry-run (write-test skipped), generous space.
    export OPT_ROOT="" OPT_FORCE=0 OPT_FORCE_FOREIGN_ESP=0 OPT_DRY_RUN=1
    export MOCK_MP_RC=0 MOCK_DF=$'Avail\n1048576'
    export VFAT_RC=0 ESP_RC=0
    is_vfat() { return "${VFAT_RC:-0}"; }
    is_esp_partition() { return "${ESP_RC:-0}"; }
}

@test "sec: generic --force does NOT bypass the ESP-type gate" {
    vesetup
    export ESP_RC=1 OPT_FORCE=1 OPT_FORCE_FOREIGN_ESP=0
    lb validate_esp /boot/efi
    [ "$status" -ne 0 ]
}

@test "flex: --force-foreign-esp is the type-gate override" {
    vesetup
    export ESP_RC=1 OPT_FORCE_FOREIGN_ESP=1
    lb validate_esp /boot/efi
    [ "$status" -eq 0 ]
}

@test "sec: non-ESP vfat with no force flags rejected" {
    vesetup
    export ESP_RC=1
    lb validate_esp /boot/efi
    [ "$status" -ne 0 ]
}

@test "flex: a real ESP passes the type gate" {
    vesetup
    export ESP_RC=0
    lb validate_esp /boot/efi
    [ "$status" -eq 0 ]
}

@test "flex: --root skips ALL host-view checks (chroot/offline image)" {
    vesetup
    # All three host-view checks would FAIL if reached; --root must skip them.
    export OPT_ROOT="/mnt" MOCK_MP_RC=1 VFAT_RC=1 ESP_RC=1
    lb validate_esp /mnt/boot/efi
    [ "$status" -eq 0 ]
}

@test "sec: --root / behaves like the live host (no bypass)" {
    vesetup
    export OPT_ROOT="/" MOCK_MP_RC=1
    lb validate_esp /boot/efi
    [ "$status" -ne 0 ]
}

@test "sec: not a mountpoint on a live install is rejected" {
    vesetup
    export MOCK_MP_RC=1
    lb validate_esp /boot/efi
    [ "$status" -ne 0 ]
}

@test "sec: a mountpoint that is not vfat is rejected" {
    vesetup
    export VFAT_RC=1
    lb validate_esp /boot
    [ "$status" -ne 0 ]
}

@test "sec: --dry-run still enforces the type gate (only the write-test is skipped)" {
    vesetup
    export OPT_DRY_RUN=1 ESP_RC=1
    lb validate_esp /boot/efi
    [ "$status" -ne 0 ]
}

@test "sec: a real ESP with too little space and no --force is rejected" {
    vesetup
    export ESP_RC=0 OPT_FORCE=0 MOCK_DF=$'Avail\n1500'
    lb validate_esp /boot/efi
    [ "$status" -ne 0 ]
}

@test "flex: --force overrides the SPACE gate on a real ESP (but never the type gate)" {
    vesetup
    export ESP_RC=0 OPT_FORCE=1 MOCK_DF=$'Avail\n1500'
    lb validate_esp /boot/efi
    [ "$status" -eq 0 ]
}
