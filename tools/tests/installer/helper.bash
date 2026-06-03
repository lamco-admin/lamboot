# Shared bats helper for the lamboot-install unit suite.
#
# Loads the installer's FUNCTIONS (via the sourcing guard added to
# tools/lamboot-install — when sourced, it skips arg dispatch / the root check /
# main) so each pure function can be exercised in isolation, and provides a
# minimal mock-command mechanism so functions that shell out to efibootmgr /
# file / lsblk / etc. see canned output.
#
# This is a DUAL-CONTRACT suite: each function carries both "must-reject"
# (security) and "must-accept across the support matrix" (flexibility — distros,
# firmware/efibootmgr versions, ESP layouts, update/upgrade scenarios).

LAMBOOT_INSTALL="${BATS_TEST_DIRNAME}/../../lamboot-install"

# Source the installer's definitions. The script sets `set -uo pipefail` at load
# time; relax it afterwards so bats' own infrastructure (which references some
# unset vars) is unaffected. The functions under test use `local` throughout and
# are exercised explicitly, so this does not change what they do.
load_installer() {
    # shellcheck disable=SC1090
    source "$LAMBOOT_INSTALL"
    set +u +o pipefail
    # The installer's top-level `declare -A MANIFEST_HASHES` / `declare -a ...`
    # run while sourced INSIDE bats' setup() function, so they create
    # function-local arrays that vanish when setup() returns — and in the test
    # body MANIFEST_HASHES would then be a plain INDEXED array, making
    # read_manifest's `MANIFEST_HASHES["EFI/LamBoot/..."]=` evaluate the slashed
    # key as arithmetic and abort the shell. Re-declare them as true globals with
    # the right types (empty here, so no data loss) to match how the installer
    # behaves when executed (not sourced) in production.
    declare -gA MANIFEST_HASHES
    declare -ga MANIFEST_ENTRIES
}

# Prepend a private mock-command directory to PATH. Call once in setup().
setup_mock_path() {
    MOCK_BIN="${BATS_TEST_TMPDIR}/mockbin"
    mkdir -p "$MOCK_BIN"
    PATH="${MOCK_BIN}:${PATH}"
    export PATH
}

# mock_stdout_var NAME ENVVAR — install a mock command `NAME` that prints the
# (possibly multi-line, tab-bearing) contents of environment variable ENVVAR
# verbatim and exits 0. The test sets ENVVAR per case. Using an env var avoids
# the quoting hazards of embedding fixtures into the mock script body.
mock_stdout_var() {
    local name="$1" envvar="$2"
    cat > "${MOCK_BIN}/${name}" <<EOF
#!/usr/bin/env bash
[ -n "\${${envvar}-}" ] && printf '%s\n' "\${${envvar}}"
exit 0
EOF
    chmod +x "${MOCK_BIN}/${name}"
}

# mock_rc NAME ENVVAR — install a mock command `NAME` that ignores its args and
# exits with the numeric return code held in environment variable ENVVAR
# (default 0 when unset/empty). For commands the function-under-test branches on
# by exit status rather than stdout (mountpoint -q, etc.).
mock_rc() {
    local name="$1" envvar="$2"
    cat > "${MOCK_BIN}/${name}" <<EOF
#!/usr/bin/env bash
exit "\${${envvar}:-0}"
EOF
    chmod +x "${MOCK_BIN}/${name}"
}

# mock_absent NAME — shadow a real command so `command -v NAME` fails and the
# function-under-test takes its tool-missing degradation path. Implemented by
# pointing NAME at a non-executable stub is unreliable; instead we rely on a
# clean MOCK_BIN-only PATH set by the caller for that case.
# (For "tool absent" cases, tests set PATH="$MOCK_BIN" with only the needed
# mocks present, so an un-mocked tool genuinely is not found.)

# lb FUNC [args...] — call a sourced installer function, capturing stdout into
# $output and exit status into $status. Use this instead of bats' `run`: the
# installer defines its own run() (the dry-run wrapper), which shadows bats' run
# when the script is sourced.
lb() {
    # `if` consumes the non-zero exit so bats' per-test errexit does not abort
    # the body the instant the function-under-test legitimately returns non-zero
    # (e.g. a security reject). Both branches are assignments (exit 0), so lb
    # itself always returns 0; the real assertion is on $status afterwards.
    if output="$("$@" 2>/dev/null)"; then status=0; else status=$?; fi
}

# lbq FUNC [args...] — like lb, but runs FUNC IN THE CURRENT SHELL (not a
# command-substitution subshell), so global side-effects (MANIFEST_HASHES,
# MANIFEST_VERSION, etc.) persist for assertions afterwards. Placing FUNC in an
# `&&`/`||` list also suppresses bats' per-test errexit INSIDE FUNC, matching
# production (the installer runs without `set -e`); a function written for that
# environment would otherwise abort under bats on an internal non-zero command
# (e.g. read_manifest's `while read` reaching EOF). $status holds the real rc.
lbq() {
    "$@" && status=0 || status=$?
    return 0
}

# restrict_tools_excluding TOOL... — rebuild the REAL-tool portion of PATH as a
# curated toolbox of symlinks that EXCLUDES the named commands, so
# `command -v <name>` genuinely fails and the function-under-test takes its
# tool-missing degradation path (BusyBox/Alpine rescue, minimal container).
# MOCK_BIN (from setup_mock_path) is kept in front so explicit mocks still win —
# e.g. you can exclude the real `file` from the toolbox AND install a mock
# `file` that reports PE32+, or exclude it entirely to force the od fallback.
# The toolbox lists every coreutil the installer's pure functions actually call.
restrict_tools_excluding() {
    local d="${BATS_TEST_TMPDIR}/toolbox"
    rm -rf "$d"; mkdir -p "$d"
    local excl=" $* "
    local t p
    for t in bash sh env od tr grep sed head tail cat printf cut sort uniq \
             sha256sum stat df findmnt lsblk mountpoint file strings iconv \
             chmod mkdir rm ln seq date dirname basename readlink; do
        case "$excl" in *" $t "*) continue ;; esac
        p=$(command -v "$t" 2>/dev/null) && ln -sf "$p" "$d/$t"
    done
    if [ -n "${MOCK_BIN:-}" ]; then PATH="${MOCK_BIN}:${d}"; else PATH="$d"; fi
    export PATH
}
