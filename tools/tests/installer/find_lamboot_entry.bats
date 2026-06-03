#!/usr/bin/env bats
#
# Dual contract for find_lamboot_entry: it must MATCH a genuine LamBoot NVRAM
# entry across every efibootmgr output format LamBoot meets in the field
# (flexibility), and must REJECT entries that merely resemble one (security:
# matching the wrong entry could delete/target it). The v0.14.0 regressions this
# locks down: the EOL anchor that broke on the \t-device-path suffix, and the
# `\*\{0,1\}` flag that missed INACTIVE entries.

load helper

setup() {
    setup_mock_path
    mock_stdout_var efibootmgr MOCK_EBM
    load_installer
}

# ---- flexibility: real entries across efibootmgr / firmware variety ----

@test "flex: active entry with \\t device-path suffix (modern efibootmgr)" {
    export MOCK_EBM=$'BootCurrent: 0001\nBoot0001* LamBoot\tHD(1,GPT,26ee4f6c)/File(\\EFI\\LamBoot\\shimx64.efi)\nBoot0002* ubuntu'
    lb find_lamboot_entry
    [ "$output" = "0001" ]
}

@test "flex: active entry, no device-path (older efibootmgr)" {
    export MOCK_EBM=$'Boot0002* LamBoot'
    lb find_lamboot_entry
    [ "$output" = "0002" ]
}

@test "flex: INACTIVE entry (two spaces, %c=space) with path" {
    export MOCK_EBM=$'Boot0003  LamBoot\tHD(1,GPT)/File(\\EFI\\LamBoot\\shimx64.efi)'
    lb find_lamboot_entry
    [ "$output" = "0003" ]
}

@test "flex: INACTIVE entry, no path" {
    export MOCK_EBM=$'Boot00A3  LamBoot'
    lb find_lamboot_entry
    [ "$output" = "00A3" ]
}

@test "flex: trailing spaces after the label tolerated" {
    export MOCK_EBM=$'Boot0005* LamBoot   '
    lb find_lamboot_entry
    [ "$output" = "0005" ]
}

@test "flex: the exact entry is picked from among decoys" {
    export MOCK_EBM=$'Boot0001* ubuntu\nBoot0002* My LamBoot Backup\nBoot0003* LamBoot\tHD(1)/File(x)\nBoot0004* Windows Boot Manager'
    lb find_lamboot_entry
    [ "$output" = "0003" ]
}

# ---- security: look-alikes must NOT match ----

@test "sec: renamed 'LamBoot Recovery' is not our entry" {
    export MOCK_EBM=$'Boot0004* LamBoot Recovery\tHD(1)/File(x)'
    lb find_lamboot_entry
    [ -z "$output" ]
}

@test "sec: 'LamBoot (old)' is not our entry" {
    export MOCK_EBM=$'Boot0007* LamBoot (old)'
    lb find_lamboot_entry
    [ -z "$output" ]
}

@test "sec: a foreign entry whose loader PATH contains \\EFI\\LamBoot\\ is not ours" {
    export MOCK_EBM=$'Boot0006* ubuntu\tHD(1)/File(\\EFI\\LamBoot\\shimx64.efi)'
    lb find_lamboot_entry
    [ -z "$output" ]
}

@test "sec: lowercase 'lamboot' does not match (exact label)" {
    export MOCK_EBM=$'Boot0008* lamboot\tHD(1)/File(x)'
    lb find_lamboot_entry
    [ -z "$output" ]
}

@test "flex: BIOS host / empty efibootmgr yields empty" {
    export MOCK_EBM=''
    lb find_lamboot_entry
    [ -z "$output" ]
}
