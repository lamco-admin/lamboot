#!/usr/bin/env bats
#
# identify_bootloader classifies the loader already present at a path by its
# interior strings, so install_fallback knows whether \EFI\BOOT\BOOTX64.EFI is
# a foreign loader (refuse to overwrite without --replace-fallback) or LamBoot's
# own (safe to refresh on upgrade).
#
# The load-bearing case (v0.16.2 regression fix): LamBoot's OWN binary embeds
# the string "systemd-boot" — its SecurityOverride mirrors systemd-boot's
# ShimLock dance. So the LamBoot self-check MUST run before the systemd-boot
# check; otherwise an `--update --fallback` upgrade misclassifies our own old
# fallback as systemd-boot and aborts, leaving the upgrade stranded (observed
# live on Tumbleweed / VM 102 with the 0.11.18 fallback).
#
# identify_bootloader only greps the file for markers, so the fixtures are plain
# files carrying the relevant strings. Functions are called directly (the
# helper sources the installer); `run` is avoided per the helper's guidance.

load helper

setup() {
    setup_mock_path
    load_installer
}

@test "sec: a LamBoot binary that ALSO contains 'systemd-boot' classifies as lamboot" {
    local f="${BATS_TEST_TMPDIR}/lamboot.efi"
    printf 'systemd-boot\nLamBoot 0.16.2\n' > "$f"   # SecurityOverride embeds systemd-boot
    [ "$(identify_bootloader "$f")" = "lamboot" ]
}

@test "flex: a genuine systemd-boot (no LamBoot marker) still classifies as systemd-boot" {
    local f="${BATS_TEST_TMPDIR}/sdboot.efi"
    printf 'systemd-boot systemd-stub\n' > "$f"
    [ "$(identify_bootloader "$f")" = "systemd-boot" ]
}

@test "flex: a genuine shim (MokManager) still classifies as shim" {
    local f="${BATS_TEST_TMPDIR}/shim.efi"
    printf 'MokManager UEFI Shim\n' > "$f"
    [ "$(identify_bootloader "$f")" = "shim" ]
}

@test "flex: a GRUB binary classifies as grub" {
    local f="${BATS_TEST_TMPDIR}/grub.efi"
    printf 'GRUB version 2.12\ngrub_main\n' > "$f"
    [ "$(identify_bootloader "$f")" = "grub" ]
}

@test "sec: a missing file classifies as none (never misread as a loader)" {
    [ "$(identify_bootloader "${BATS_TEST_TMPDIR}/does-not-exist.efi")" = "none" ]
}
