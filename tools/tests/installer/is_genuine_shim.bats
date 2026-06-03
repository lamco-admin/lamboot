#!/usr/bin/env bats
#
# Dual contract for is_genuine_shim + _is_pe_binary. The Secure Boot anchor must
# be a REAL shim: trusting a foreign loader (GRUB/systemd-boot) parked at a shim
# search path lets LamBoot chain through an unverified loader, defeating the
# shim->MOK chain (security). But REJECTING a genuine shim silently degrades an
# SB install to a direct-loader entry that won't boot on a MOK-only host — so the
# markers must accept every real shim across the matrix: modern SBAT shims,
# pre-SBAT (15.3-era, still on Ubuntu 20.04 / RHEL 8) shims authenticated only by
# their UTF-16LE MokManager handoff strings, aarch64 (mmaa64) shims, and all of
# the above on degraded hosts where `file`/`strings` are absent (flexibility).
#
# The v0.14.0 regressions this locks down: marker 2 grepping UTF-16 as ASCII
# (dead — fixed with `tr -d '\0'`), and a hard reject when `file` was absent
# (fixed with the od MZ/PE header fallback).

load helper

setup() {
    setup_mock_path
}

# ---------------------------------------------------------------------------
# Fixture builders
# ---------------------------------------------------------------------------

# build_pe OUT — write a minimal but VALID PE header that _is_pe_binary's od
# fallback accepts: `MZ` at 0, e_lfanew=0x80 (uint32 LE) at offset 60, `PE\0\0`
# at offset 128. Marker bytes are appended afterwards by the caller.
build_pe() {
    local out="$1"
    {
        printf 'MZ'                      # 0..1   DOS magic
        printf '\0%.0s' $(seq 1 58)      # 2..59  pad to e_lfanew field
        printf '\200\0\0\0'              # 60..63 e_lfanew = 0x00000080 = 128 (LE)
        printf '\0%.0s' $(seq 1 64)      # 64..127 pad to the PE signature
        printf 'PE\0\0'                  # 128..131 PE signature
    } > "$out"
}

# append_ascii FILE STR — append literal ASCII bytes (an SBAT row, a planted
# string, etc.) with a trailing newline.
append_ascii() { printf '%s\n' "$2" >> "$1"; }

# append_utf16le FILE STR — append STR as UTF-16LE (CHAR16), the encoding shim
# uses for its MokManager / mm{x64,aa64}.efi handoff strings. grep -a cannot see
# these (bytes are m\0m\0...); marker 2's `tr -d '\0'` is what recovers them.
append_utf16le() { printf '%s' "$2" | iconv -f ASCII -t UTF-16LE >> "$1"; }

# mock the system `file` command to report a chosen classification regardless of
# argument, so the file-present path of _is_pe_binary is deterministic.
mock_file_reports() { export MOCK_FILE="$1"; mock_stdout_var file MOCK_FILE; }

# ===========================================================================
# flexibility — every real shim across the matrix must be accepted
# ===========================================================================

@test "flex: modern SBAT shim (file present, marker 1)" {
    mock_file_reports 'shimx64.efi: PE32+ executable (EFI application)'
    load_installer
    build_pe "$BATS_TEST_TMPDIR/shim.efi"
    append_ascii "$BATS_TEST_TMPDIR/shim.efi" 'shim,4,UEFI shim,shim,15.8,https://github.com/rhboot/shim'
    lb is_genuine_shim "$BATS_TEST_TMPDIR/shim.efi"
    [ "$status" -eq 0 ]
}

@test "flex: multi-digit SBAT generation accepted (shim,12,UEFI shim)" {
    mock_file_reports 'PE32+ executable'
    load_installer
    build_pe "$BATS_TEST_TMPDIR/shim.efi"
    append_ascii "$BATS_TEST_TMPDIR/shim.efi" 'shim,12,UEFI shim,shim,15.9,url'
    lb is_genuine_shim "$BATS_TEST_TMPDIR/shim.efi"
    [ "$status" -eq 0 ]
}

@test "flex: pre-SBAT shim via UTF-16LE MokManager handoff (marker 2)" {
    mock_file_reports 'PE32+ executable'
    load_installer
    build_pe "$BATS_TEST_TMPDIR/shim.efi"
    append_utf16le "$BATS_TEST_TMPDIR/shim.efi" 'mmx64.efi'
    append_utf16le "$BATS_TEST_TMPDIR/shim.efi" 'MokManager'
    lb is_genuine_shim "$BATS_TEST_TMPDIR/shim.efi"
    [ "$status" -eq 0 ]
}

@test "flex: pre-SBAT shim matches with no \`strings\` tool (marker 2 is strings-free)" {
    mock_file_reports 'PE32+ executable'
    restrict_tools_excluding strings
    load_installer
    build_pe "$BATS_TEST_TMPDIR/shim.efi"
    append_utf16le "$BATS_TEST_TMPDIR/shim.efi" 'MokManager'
    lb is_genuine_shim "$BATS_TEST_TMPDIR/shim.efi"
    [ "$status" -eq 0 ]
}

@test "flex: modern shim on a host WITHOUT \`file\` (od fallback validates PE, marker 1)" {
    restrict_tools_excluding file
    load_installer
    build_pe "$BATS_TEST_TMPDIR/shim.efi"
    append_ascii "$BATS_TEST_TMPDIR/shim.efi" 'shim,4,UEFI shim,shim,15.8,url'
    lb is_genuine_shim "$BATS_TEST_TMPDIR/shim.efi"
    [ "$status" -eq 0 ]
}

@test "flex: pre-SBAT shim on a host without \`file\` AND without \`strings\` (od + tr)" {
    restrict_tools_excluding file strings
    load_installer
    build_pe "$BATS_TEST_TMPDIR/shim.efi"
    append_utf16le "$BATS_TEST_TMPDIR/shim.efi" 'MokManager'
    lb is_genuine_shim "$BATS_TEST_TMPDIR/shim.efi"
    [ "$status" -eq 0 ]
}

@test "flex: aarch64 shim via mmaa64.efi marker" {
    mock_file_reports 'PE32+ executable (EFI application aarch64)'
    load_installer
    build_pe "$BATS_TEST_TMPDIR/shimaa64.efi"
    append_utf16le "$BATS_TEST_TMPDIR/shimaa64.efi" 'mmaa64.efi'
    lb is_genuine_shim "$BATS_TEST_TMPDIR/shimaa64.efi"
    [ "$status" -eq 0 ]
}

@test "flex: ASCII MokManager copy also matches (tr is a no-op on ASCII)" {
    mock_file_reports 'PE32+ executable'
    load_installer
    build_pe "$BATS_TEST_TMPDIR/shim.efi"
    append_ascii "$BATS_TEST_TMPDIR/shim.efi" 'some vendor MokManager build'
    lb is_genuine_shim "$BATS_TEST_TMPDIR/shim.efi"
    [ "$status" -eq 0 ]
}

# ===========================================================================
# security — look-alikes parked at shim paths must be rejected
# ===========================================================================

@test "sec: distro GRUB (grub SBAT, no shim row, no MokManager) is rejected" {
    mock_file_reports 'grubx64.efi: PE32+ executable'
    load_installer
    build_pe "$BATS_TEST_TMPDIR/grubx64.efi"
    append_ascii "$BATS_TEST_TMPDIR/grubx64.efi" 'grub,3,Free Software Foundation,grub,2.06,url'
    lb is_genuine_shim "$BATS_TEST_TMPDIR/grubx64.efi"
    [ "$status" -eq 1 ]
}

@test "sec: GRUB carrying the shim_lock verifier module name is NOT a shim" {
    mock_file_reports 'PE32+ executable'
    load_installer
    build_pe "$BATS_TEST_TMPDIR/grubx64.efi"
    # 'shim_lock' is GRUB's verifier module — must not satisfy 'shim,N,UEFI shim'.
    append_ascii "$BATS_TEST_TMPDIR/grubx64.efi" 'shim_lock'
    append_ascii "$BATS_TEST_TMPDIR/grubx64.efi" 'grub,3,Free Software Foundation,grub,2.12,url'
    lb is_genuine_shim "$BATS_TEST_TMPDIR/grubx64.efi"
    [ "$status" -eq 1 ]
}

@test "sec: systemd-boot is a loader but not a shim — rejected" {
    mock_file_reports 'PE32+ executable'
    load_installer
    build_pe "$BATS_TEST_TMPDIR/systemd-bootx64.efi"
    append_ascii "$BATS_TEST_TMPDIR/systemd-bootx64.efi" 'systemd-boot,1,systemd,systemd-boot,254,url'
    lb is_genuine_shim "$BATS_TEST_TMPDIR/systemd-bootx64.efi"
    [ "$status" -eq 1 ]
}

@test "sec: valid PE32+ with NONE of the shim markers (rEFInd/stub/LamBoot) is rejected" {
    mock_file_reports 'PE32+ executable'
    load_installer
    build_pe "$BATS_TEST_TMPDIR/refind.efi"
    append_ascii "$BATS_TEST_TMPDIR/refind.efi" 'refind_x64 a graphical boot manager'
    lb is_genuine_shim "$BATS_TEST_TMPDIR/refind.efi"
    [ "$status" -eq 1 ]
}

@test "sec: non-PE file is rejected even if it contains the SBAT string (PE gate first, file present)" {
    mock_file_reports 'script.sh: a /bin/sh script, ASCII text executable'
    load_installer
    printf '#!/bin/sh\nshim,4,UEFI shim\n' > "$BATS_TEST_TMPDIR/evil.sh"
    lb is_genuine_shim "$BATS_TEST_TMPDIR/evil.sh"
    [ "$status" -eq 1 ]
}

@test "sec: non-PE file rejected under od fallback (no MZ) even with planted SBAT string" {
    restrict_tools_excluding file
    load_installer
    printf '#!/bin/sh\nshim,4,UEFI shim\n' > "$BATS_TEST_TMPDIR/evil.sh"
    lb is_genuine_shim "$BATS_TEST_TMPDIR/evil.sh"
    [ "$status" -eq 1 ]
}

@test "sec: missing path and directory path are rejected at the -f guard" {
    mock_file_reports 'PE32+ executable'
    load_installer
    lb is_genuine_shim "$BATS_TEST_TMPDIR/does-not-exist.efi"
    [ "$status" -eq 1 ]
    mkdir -p "$BATS_TEST_TMPDIR/adir"
    lb is_genuine_shim "$BATS_TEST_TMPDIR/adir"
    [ "$status" -eq 1 ]
}

@test "sec: truncated PE (MZ only, e_lfanew past EOF) rejected under od fallback" {
    restrict_tools_excluding file
    load_installer
    printf 'MZ\0\0\0\0\0\0' > "$BATS_TEST_TMPDIR/trunc.efi"
    lb is_genuine_shim "$BATS_TEST_TMPDIR/trunc.efi"
    [ "$status" -eq 1 ]
}
