#!/usr/bin/env bats
#
# Dual contract for BLS entry placement (v0.15.0 read-in-place).
#
# flexibility: when /boot is a separate partition LamBoot reads natively (vfat
#   via the FatRo reader, ext4/btrfs natively), entries go ON /boot so the
#   kernel is sourced in place — no ESP kernel mirror; the manifest records the
#   off-ESP location with a `boot:` prefix.
# security: a non-native separate /boot (xfs/f2fs/zfs) or /boot-is-the-ESP must
#   fall back to ESP staging — never silently place entries on a volume LamBoot
#   cannot read, which would yield an empty menu.

load helper

setup() {
    setup_mock_path
    load_installer
    set +u
    ESP="${BATS_TEST_TMPDIR}/esp"
    mkdir -p "${ESP}/${BLS_DIR}"
    MANIFEST_ENTRIES=()
    OPT_ROOT=""
    OPT_JSON=0
    BLS_INSTALL_DIR=""
    BLS_ON_BOOT=0
    BLS_PLACEMENT="esp"
}

# Install an arg-aware findmnt/mountpoint pair driven by env:
#   BOOT_IS_MOUNT=1|0, BOOT_SRC, ESP_SRC, BOOT_FS
mock_boot_layout() {
    cat > "${MOCK_BIN}/mountpoint" <<EOF
#!/usr/bin/env bash
# args: -q <path>
for a in "\$@"; do case "\$a" in /boot) [ "\${BOOT_IS_MOUNT}" = "1" ] && exit 0 || exit 1;; esac; done
exit 1
EOF
    cat > "${MOCK_BIN}/findmnt" <<EOF
#!/usr/bin/env bash
# findmnt -n -o <FIELD> <PATH>
field=""; path=""
while [ \$# -gt 0 ]; do case "\$1" in -o) field="\$2"; shift 2;; -n) shift;; *) path="\$1"; shift;; esac; done
case "\${field}:\${path}" in
    SOURCE:/boot) printf '%s\n' "\${BOOT_SRC}";;
    SOURCE:${ESP}) printf '%s\n' "\${ESP_SRC}";;
    FSTYPE:/boot) printf '%s\n' "\${BOOT_FS}";;
esac
EOF
    chmod +x "${MOCK_BIN}/mountpoint" "${MOCK_BIN}/findmnt"
}

# ---------------------------------------------------------------------------
# detect_bls_target
# ---------------------------------------------------------------------------

@test "flex: separate vfat /boot -> read-in-place on /boot (FatRo)" {
    mock_boot_layout
    BOOT_IS_MOUNT=1 BOOT_SRC=/dev/sda1 ESP_SRC=/dev/sda3 BOOT_FS=vfat lbq detect_bls_target
    [ "$BLS_ON_BOOT" = "1" ]
    [ "$BLS_INSTALL_DIR" = "/boot/${BLS_DIR}" ]
    [ "$BLS_PLACEMENT" = "boot_in_place" ]
}

@test "flex: separate ext4 /boot -> read-in-place on /boot" {
    mock_boot_layout
    BOOT_IS_MOUNT=1 BOOT_SRC=/dev/sda1 ESP_SRC=/dev/sda3 BOOT_FS=ext4 lbq detect_bls_target
    [ "$BLS_ON_BOOT" = "1" ]
    [ "$BLS_INSTALL_DIR" = "/boot/${BLS_DIR}" ]
}

@test "sec: separate xfs /boot (non-native) -> ESP-staged fallback" {
    mock_boot_layout
    BOOT_IS_MOUNT=1 BOOT_SRC=/dev/sda1 ESP_SRC=/dev/sda3 BOOT_FS=xfs lbq detect_bls_target
    [ "$BLS_ON_BOOT" = "0" ]
    [ "$BLS_INSTALL_DIR" = "${ESP}/${BLS_DIR}" ]
}

@test "sec: /boot not a separate mount -> ESP-staged" {
    mock_boot_layout
    BOOT_IS_MOUNT=0 lbq detect_bls_target
    [ "$BLS_ON_BOOT" = "0" ]
    [ "$BLS_INSTALL_DIR" = "${ESP}/${BLS_DIR}" ]
}

@test "sec: /boot source == ESP source (boot is the ESP) -> ESP-staged" {
    mock_boot_layout
    BOOT_IS_MOUNT=1 BOOT_SRC=/dev/sda1 ESP_SRC=/dev/sda1 BOOT_FS=vfat lbq detect_bls_target
    [ "$BLS_ON_BOOT" = "0" ]
}

@test "sec: --root/chroot never infers read-in-place from the host mount table" {
    mock_boot_layout
    OPT_ROOT="/mnt/target"
    BOOT_IS_MOUNT=1 BOOT_SRC=/dev/sda1 ESP_SRC=/dev/sda3 BOOT_FS=vfat lbq detect_bls_target
    [ "$BLS_ON_BOOT" = "0" ]
}

# ---------------------------------------------------------------------------
# manifest_add_bls — location-aware manifest rows
# ---------------------------------------------------------------------------

@test "flex: ESP-staged entry recorded with the plain ESP-relative path" {
    BLS_ON_BOOT=0
    BLS_INSTALL_DIR="${ESP}/${BLS_DIR}"
    printf 'x' > "${ESP}/${BLS_DIR}/arch-1.conf"
    lbq manifest_add_bls "arch-1.conf"
    # last manifest row ends with the ESP-relative path, no boot: prefix
    printf '%s\n' "${MANIFEST_ENTRIES[@]}" | grep -qE "  ${BLS_DIR}/arch-1.conf\$"
    ! printf '%s\n' "${MANIFEST_ENTRIES[@]}" | grep -q "boot:"
}

@test "flex: read-in-place entry recorded with a boot: prefix (still grep-matchable)" {
    BLS_ON_BOOT=1
    BLS_INSTALL_DIR="${BATS_TEST_TMPDIR}/boot/${BLS_DIR}"
    mkdir -p "$BLS_INSTALL_DIR"
    printf 'x' > "${BLS_INSTALL_DIR}/arch-1.conf"
    lbq manifest_add_bls "arch-1.conf"
    printf '%s\n' "${MANIFEST_ENTRIES[@]}" | grep -qE "  boot:${BLS_DIR}/arch-1.conf\$"
    # the substring loader/entries/arch-1.conf still matches, so the existing
    # stale/removal greps keep working
    printf '%s\n' "${MANIFEST_ENTRIES[@]}" | grep -qF "${BLS_DIR}/arch-1.conf"
}
