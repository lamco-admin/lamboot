#!/usr/bin/env bash
# sync-versions.sh — propagate the lamboot umbrella version (and optionally the
# installer protocol version) across every site in this repo that stamps one.
#
# Why this exists: between v0.9.0 and v0.10.0 the installer script's
# LAMBOOT_VERSION constant drifted five releases out of sync because each
# release ceremony required hand-editing six different files. This script
# makes that drift impossible by treating Cargo.toml as the canonical source
# and rewriting every other site to match.
#
# Two version numbers exist in this repo (only two — by design):
#
#   1. LAMBOOT_VERSION (umbrella) — semver MAJOR.MINOR.PATCH for the whole
#      release. Bumps on every shipped release. Stamped in 6 places.
#      Source of truth: Cargo.toml workspace `version = "..."`.
#
#   2. LAMBOOT_INSTALLER_PROTOCOL_VERSION (semantic) — single integer that
#      bumps ONLY on backward-incompatible changes to the installer JSON
#      protocol consumed by Calamares/archinstall/openSUSE. Independent
#      of umbrella version. Source of truth: tools/lamboot-install.
#
# Component tools (lamboot-install, lamboot-sign, lamboot-inspect) do NOT
# have their own versions. They carry the umbrella version. See the
# rationale in docs/specs/SPEC-LAMBOOT-INSTALLER-PROTOCOL-V1.md.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CARGO_TOML="${REPO_ROOT}/Cargo.toml"

# ============================================================================
# Sites that stamp the umbrella version. Each entry is:
#   "PATH|GREP-PATTERN-MATCHING-CURRENT|SED-EXPR-TEMPLATE"
# where SED-EXPR-TEMPLATE has %VERSION% substituted at runtime.
# Sites are checked in --check mode and rewritten in --sync / --bump modes.
# ============================================================================

UMBRELLA_SITES=(
    # Cargo.toml workspace version — canonical source.
    "Cargo.toml|^version = \"[0-9]+\.[0-9]+\.[0-9]+\"|s|^version = \"[0-9]+\.[0-9]+\.[0-9]+\"|version = \"%VERSION%\"|"

    # tools/lamboot-install — readonly LAMBOOT_VERSION="..."
    "tools/lamboot-install|^readonly LAMBOOT_VERSION=\"[0-9]+\.[0-9]+\.[0-9]+\"|s|^readonly LAMBOOT_VERSION=\"[0-9]+\.[0-9]+\.[0-9]+\"|readonly LAMBOOT_VERSION=\"%VERSION%\"|"

    # AUR is intentionally NOT swept here — packaging is decoupled
    # (PACKAGING.md §4): per-channel revisions + the publish-time render from
    # packaging/release.toml own the packaged version, not the umbrella sweep.
    # The 2026-06 restructure moved aur/lamboot -> aur/lamboot-bin; re-coupling
    # a per-pkgrel channel to this sweep would contradict the decoupled model.

    # archinstall plugin — __plugin_version__ = "..."
    "packaging/installers/archinstall/lamboot_plugin.py|^__plugin_version__ = \"[0-9]+\.[0-9]+\.[0-9]+\"|s|^__plugin_version__ = \"[0-9]+\.[0-9]+\.[0-9]+\"|__plugin_version__ = \"%VERSION%\"|"

    # Calamares plugin — __plugin_version__ = "..."
    "packaging/installers/calamares/lamboot/main.py|^__plugin_version__ = \"[0-9]+\.[0-9]+\.[0-9]+\"|s|^__plugin_version__ = \"[0-9]+\.[0-9]+\.[0-9]+\"|__plugin_version__ = \"%VERSION%\"|"
)

# VESTIGIAL (2026-06: packaging decoupled — PACKAGING.md §4). The AUR .SRCINFO
# is no longer umbrella-swept; the regenerate_srcinfo/check_srcinfo helpers
# below are retained but unreferenced. Do NOT re-wire them into the umbrella
# sweep — packaging versions are owned by packaging/release.toml + the
# publish-time render. Kept (not deleted) so the recipe survives a policy revert.
SRCINFO_PATH="packaging/aur/lamboot/.SRCINFO"

# ============================================================================
# Protocol version sites (just one — single source of truth)
# ============================================================================

PROTOCOL_SITES=(
    "tools/lamboot-install|^readonly LAMBOOT_INSTALLER_PROTOCOL_VERSION=[0-9]+|s|^readonly LAMBOOT_INSTALLER_PROTOCOL_VERSION=[0-9]+|readonly LAMBOOT_INSTALLER_PROTOCOL_VERSION=%VERSION%|"
)

# ============================================================================
# Helpers
# ============================================================================

usage() {
    cat <<'HELP_EOF'
sync-versions.sh — stamp lamboot umbrella + protocol versions across the repo.

Usage:
    tools/sync-versions.sh                  # alias for --sync
    tools/sync-versions.sh --check          # verify; exit 1 on drift
    tools/sync-versions.sh --sync           # rewrite all sites to match Cargo.toml
    tools/sync-versions.sh --bump X.Y.Z     # set Cargo.toml AND all sites to X.Y.Z
    tools/sync-versions.sh --bump-protocol N  # set protocol version to N
    tools/sync-versions.sh --bump-sbat-generation  # increment SBAT generation (revocation; rare)
    tools/sync-versions.sh --dry-run --bump X.Y.Z  # preview without writing

Environment:
    Treats Cargo.toml as the source of truth for the umbrella version.
    Treats tools/lamboot-install LAMBOOT_INSTALLER_PROTOCOL_VERSION as the
    source of truth for the protocol version.

Exit codes:
    0  success / no drift
    1  drift detected (--check) or error
    2  bad arguments
HELP_EOF
}

die() { echo "sync-versions: ERROR: $*" >&2; exit 1; }
warn() { echo "sync-versions: warn: $*" >&2; }
note() { echo "sync-versions: $*"; }

valid_semver() {
    [[ "$1" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[a-z0-9.]+)?$ ]]
}

valid_protocol() {
    [[ "$1" =~ ^[1-9][0-9]*$ ]]
}

# Read Cargo.toml workspace version. Single regex; we don't pull a TOML
# parser in just for one field.
read_cargo_version() {
    awk '/^version = "[0-9]+\.[0-9]+\.[0-9]+"/ { gsub(/version = "|"/, "", $0); print; exit }' "$CARGO_TOML"
}

# Read protocol version from lamboot-install (the source of truth for it).
read_protocol_version() {
    awk '/^readonly LAMBOOT_INSTALLER_PROTOCOL_VERSION=/ {
        gsub(/readonly LAMBOOT_INSTALLER_PROTOCOL_VERSION=/, "", $0); print; exit
    }' "${REPO_ROOT}/tools/lamboot-install"
}

# Check that every site in SITES has the expected version stamp. Returns
# the count of drifted sites via global DRIFT_COUNT and prints diff to stderr.
DRIFT_COUNT=0
DRIFT_FILES=()
check_sites() {
    local expected="$1"; shift
    local label="$1"; shift
    local -n _sites=$1
    DRIFT_COUNT=0
    DRIFT_FILES=()
    local entry path grep_pat
    for entry in "${_sites[@]}"; do
        IFS='|' read -r path grep_pat _ _ _ <<< "$entry"
        local full="${REPO_ROOT}/${path}"
        if [[ ! -f "$full" ]]; then
            warn "[${label}] missing: ${path}"
            DRIFT_COUNT=$((DRIFT_COUNT+1))
            DRIFT_FILES+=("$path")
            continue
        fi
        local current_line
        current_line=$(grep -E "$grep_pat" "$full" | head -1 || true)
        if [[ -z "$current_line" ]]; then
            warn "[${label}] no version line found in ${path}"
            DRIFT_COUNT=$((DRIFT_COUNT+1))
            DRIFT_FILES+=("$path")
            continue
        fi
        if ! grep -qE "${expected//./\\.}" <<< "$current_line"; then
            note "[${label}] DRIFT ${path}: '${current_line//[[:space:]]/ }' (expected ${expected})"
            DRIFT_COUNT=$((DRIFT_COUNT+1))
            DRIFT_FILES+=("$path")
        fi
    done
}

# Rewrite every site in SITES to use the new version. Honors DRY_RUN.
rewrite_sites() {
    local new_ver="$1"; shift
    local label="$1"; shift
    local -n _sites=$1
    local entry path _ sed_l sed_search sed_replace _trail
    for entry in "${_sites[@]}"; do
        IFS='|' read -r path _ sed_l sed_search sed_replace _trail <<< "$entry"
        local full="${REPO_ROOT}/${path}"
        [[ -f "$full" ]] || { warn "[${label}] missing: ${path}"; continue; }
        local final_replace="${sed_replace//%VERSION%/${new_ver}}"
        local sed_expr="${sed_l}|${sed_search}|${final_replace}|"
        if (( DRY_RUN )); then
            note "[${label}] [dry-run] would rewrite ${path}: ${sed_expr}"
        else
            sed -i -E -e "$sed_expr" "$full"
            note "[${label}] rewrote ${path} -> ${new_ver}"
        fi
    done
}

regenerate_srcinfo() {
    local full="${REPO_ROOT}/${SRCINFO_PATH}"
    [[ -f "$full" ]] || { warn "missing: ${SRCINFO_PATH}"; return; }
    local new_ver="$1"

    if (( DRY_RUN )); then
        note "[srcinfo] [dry-run] would regenerate ${SRCINFO_PATH} with pkgver=${new_ver}"
        return
    fi

    # Try makepkg first; fall back to in-place rewrite if unavailable.
    if command -v makepkg >/dev/null 2>&1; then
        ( cd "${REPO_ROOT}/packaging/aur/lamboot" && makepkg --printsrcinfo > .SRCINFO )
        note "[srcinfo] regenerated ${SRCINFO_PATH} via makepkg"
    else
        # .SRCINFO uses literal-tab indentation; sed -E lets us match it directly
        sed -i -E \
            -e "s|^	pkgver = [0-9]+\.[0-9]+\.[0-9]+\$|	pkgver = ${new_ver}|" \
            -e "s|^	provides = lamboot=[0-9]+\.[0-9]+\.[0-9]+\$|	provides = lamboot=${new_ver}|" \
            -e "s|/v[0-9]+\.[0-9]+\.[0-9]+/lamboot-[0-9]+\.[0-9]+\.[0-9]+-|/v${new_ver}/lamboot-${new_ver}-|g" \
            "$full"
        note "[srcinfo] rewrote ${SRCINFO_PATH} -> ${new_ver} (makepkg not on PATH; fallback in-place)"
    fi
}

check_srcinfo() {
    local expected="$1"
    local full="${REPO_ROOT}/${SRCINFO_PATH}"
    [[ -f "$full" ]] || { warn "missing: ${SRCINFO_PATH}"; DRIFT_COUNT=$((DRIFT_COUNT+1)); return; }
    local line
    # .SRCINFO uses literal-tab indentation, not "\t" escape
    line=$(grep -E "^	pkgver = " "$full" | head -1 || true)
    if [[ -z "$line" ]] || ! grep -qE "${expected//./\\.}" <<< "$line"; then
        note "[srcinfo] DRIFT ${SRCINFO_PATH}: '${line//[[:space:]]/ }' (expected ${expected})"
        DRIFT_COUNT=$((DRIFT_COUNT+1))
        DRIFT_FILES+=("$SRCINFO_PATH")
    fi
}

# ============================================================================
# Modes
# ============================================================================

mode_check() {
    local cargo_ver
    cargo_ver=$(read_cargo_version)
    [[ -n "$cargo_ver" ]] || die "could not read version from Cargo.toml"
    note "Cargo.toml version: ${cargo_ver}"

    local proto_ver
    proto_ver=$(read_protocol_version)
    [[ -n "$proto_ver" ]] || die "could not read protocol version from tools/lamboot-install"
    note "Protocol version:   ${proto_ver}"

    local total_drift=0

    check_sites "$cargo_ver" "umbrella" UMBRELLA_SITES
    total_drift=$((total_drift + DRIFT_COUNT))

    # Re-run for protocol (single-site check)
    local saved_drift=$total_drift
    check_sites "$proto_ver" "protocol" PROTOCOL_SITES
    # PROTOCOL_SITES uses different file, but lamboot-install is also in
    # UMBRELLA_SITES — that's fine, each grep pattern looks for a
    # different line in the file, so they don't conflict.

    if (( total_drift == 0 && DRIFT_COUNT == 0 )); then
        note "all version sites in sync (umbrella=${cargo_ver}, protocol=${proto_ver})"
        check_sbat_advisory "$cargo_ver"
        return 0
    fi
    note "DRIFT DETECTED: ${total_drift} umbrella + ${DRIFT_COUNT} protocol site(s)"
    note "Run: tools/sync-versions.sh --sync"
    return 1
}

# SBAT advisory check (NOT a hard drift signal — SBAT is governance-level
# per ~/lamco-admin/projects/lamboot/VERSIONING-POLICY.md §6). The
# version string in dist/sbat.csv column 5 is human-facing; the
# generation NUMBER in column 2 is what shim's SBAT enforcement
# actually consults for revocation. We don't auto-sweep — but we DO
# advise when the human-version drifts from the umbrella so the
# release operator sees it and makes a conscious bump (or doesn't).
check_sbat_advisory() {
    local cargo_ver="$1"
    # Prefer the tracked source; fall back to dist/ for back-compat
    # with workflows that haven't rebuilt yet.
    # REPO_ROOT-anchored so the advisory works from any working directory
    # (the script never cd's). Previously bare paths made it silently no-op
    # when run from a subdirectory.
    local sbat_csv
    if [[ -f "${REPO_ROOT}/packaging/sbat.csv" ]]; then
        sbat_csv="${REPO_ROOT}/packaging/sbat.csv"
    elif [[ -f "${REPO_ROOT}/dist/sbat.csv" ]]; then
        sbat_csv="${REPO_ROOT}/dist/sbat.csv"
    else
        return 0
    fi
    local sbat_ver
    sbat_ver=$(awk -F, '$1 == "lamboot" { print $5; exit }' "$sbat_csv")
    [[ -n "$sbat_ver" ]] || return 0
    if [[ "$sbat_ver" != "$cargo_ver" ]]; then
        printf '\n' >&2
        note "ADVISORY: ${sbat_csv} lamboot version is '${sbat_ver}'"
        note "          but Cargo.toml umbrella version is '${cargo_ver}'."
        note "          SBAT is intentionally NOT auto-swept — see"
        note "          VERSIONING-POLICY.md §5a. The version string is"
        note "          human-facing; the generation number (column 2)"
        note "          drives shim's actual revocation enforcement."
        note "          To update the version string for this release:"
        note "            sed -i 's|^lamboot,1,Lamco Development,lamboot,[0-9.]\\+,|lamboot,1,Lamco Development,lamboot,${cargo_ver},|' ${sbat_csv}"
        note "          (then re-run ./build.sh so dist/sbat.csv inherits)"
        note "          To bump the generation (security revocation; rare,"
        note "          requires re-signing all live binaries): increment"
        note "          column 2 of the lamboot row, or run --bump-sbat-generation;"
        note "          see packaging/SBAT-GENERATION.md."
    fi

    # Cross-check: if dist/sbat.csv exists and disagrees with the tracked
    # packaging/sbat.csv, surface it — usually an out-of-date build artifact
    # lingering. Compare BOTH the human-facing version (column 5) AND the SBAT
    # generation (column 2): a --bump-sbat-generation moves only column 2, so a
    # version-only check would miss a stale dist generation entirely.
    if [[ "$sbat_csv" == "${REPO_ROOT}/packaging/sbat.csv" && -f "${REPO_ROOT}/dist/sbat.csv" ]]; then
        local dist_ver dist_gen pkg_gen
        dist_ver=$(awk -F, '$1 == "lamboot" { print $5; exit }' "${REPO_ROOT}/dist/sbat.csv")
        dist_gen=$(awk -F, '$1 == "lamboot" { print $2; exit }' "${REPO_ROOT}/dist/sbat.csv")
        pkg_gen=$(awk -F, '$1 == "lamboot" { print $2; exit }' "$sbat_csv")
        if [[ -n "$dist_ver" && "$dist_ver" != "$sbat_ver" ]]; then
            printf '\n' >&2
            note "ADVISORY: dist/sbat.csv version (${dist_ver}) is stale against"
            note "          packaging/sbat.csv (${sbat_ver}). Re-run ./build.sh"
            note "          to refresh, or sign-lamboot.sh will sign the stale row."
        fi
        if [[ -n "$dist_gen" && -n "$pkg_gen" && "$dist_gen" != "$pkg_gen" ]]; then
            printf '\n' >&2
            note "ADVISORY: dist/sbat.csv SBAT generation (${dist_gen}) is stale against"
            note "          packaging/sbat.csv (${pkg_gen}) — a generation bump has not"
            note "          been built/signed yet. Re-run ./build.sh --sign so the"
            note "          revocation actually ships; until then it is INERT."
        fi
    fi
}

mode_sync() {
    local cargo_ver
    cargo_ver=$(read_cargo_version)
    [[ -n "$cargo_ver" ]] || die "could not read version from Cargo.toml"

    note "syncing all sites to Cargo.toml version: ${cargo_ver}"
    rewrite_sites "$cargo_ver" "umbrella" UMBRELLA_SITES
    note "sync complete"
}

mode_bump() {
    local new_ver="$1"
    valid_semver "$new_ver" || die "invalid version '$new_ver' (expected MAJOR.MINOR.PATCH)"

    local cur_ver
    cur_ver=$(read_cargo_version)
    note "bumping umbrella version: ${cur_ver} -> ${new_ver}"
    rewrite_sites "$new_ver" "umbrella" UMBRELLA_SITES
    sync_cargo_lock "$new_ver"
    note "bump complete"
}

# Re-resolve Cargo.lock so the workspace members' lock entries match the
# just-bumped Cargo.toml versions. Without this the lock is stale and
# `build.sh --locked` (and `cargo build --locked`) ABORT — breaking the
# documented release sequence (bump then build). Best-effort: warns rather than
# failing the bump if cargo is unavailable (e.g. a stamping-only environment) or
# offline resolution can't run, so the operator can run `cargo check` by hand.
sync_cargo_lock() {
    local new_ver="$1"
    if (( DRY_RUN )); then
        note "[dry-run] would re-resolve Cargo.lock to match the new version"
        return 0
    fi
    if ! command -v cargo >/dev/null 2>&1; then
        note "WARNING: cargo not on PATH; Cargo.lock NOT updated."
        note "         Run 'cargo check' (or 'cargo update --workspace') before"
        note "         ./build.sh --locked, or the build will abort on a stale lock."
        return 0
    fi
    # --workspace re-resolves only the workspace members (the bumped versions);
    # it does not change dependency versions. Try offline first (deps are already
    # resolved), then a normal run as a fallback.
    if ( cd "$REPO_ROOT" && cargo update --workspace --offline ) >/dev/null 2>&1 \
       || ( cd "$REPO_ROOT" && cargo update --workspace ) >/dev/null 2>&1; then
        note "Cargo.lock re-resolved to ${new_ver}"
    else
        note "WARNING: 'cargo update --workspace' failed; Cargo.lock may be stale."
        note "         Run 'cargo check' before ./build.sh --locked."
    fi
}

mode_bump_protocol() {
    local new_ver="$1"
    valid_protocol "$new_ver" || die "invalid protocol version '$new_ver' (expected positive integer)"

    local cur_ver
    cur_ver=$(read_protocol_version)
    note "bumping protocol version: ${cur_ver} -> ${new_ver}"
    rewrite_sites "$new_ver" "protocol" PROTOCOL_SITES
    note "protocol bump complete"
    note "Reminder: a protocol bump implies a NEW spec doc"
    note "  (docs/specs/SPEC-LAMBOOT-INSTALLER-PROTOCOL-V${new_ver}.md)"
}

# Increment the SBAT generation (column 2 of the `lamboot` row in
# packaging/sbat.csv) by one. This is the BootHole-class revocation lever:
# shim refuses every LamBoot signed at a generation BELOW the firmware
# SBATLevel, so bumping it revokes all prior-generation binaries. Deliberately
# auto-increment-only (revocations are sequential) and deliberately NOT part of
# --sync or a version --bump — the human-facing version (column 5) is swept
# separately; the generation moves ONLY on a real security revocation. The CSV
# edit is the trivial part; the mandatory manual follow-up (re-sign every live
# binary, coordinate SBATLevel) is printed and is what actually makes the
# revocation effective. See packaging/SBAT-GENERATION.md.
mode_bump_sbat_generation() {
    # REPO_ROOT-anchored like every other file access in this script (it never
    # cd's), so the mode works from any working directory.
    local sbat_csv="${REPO_ROOT}/packaging/sbat.csv"
    [[ -f "$sbat_csv" ]] || die "SBAT CSV not found: ${sbat_csv}"

    local cur_gen
    cur_gen=$(awk -F, '$1 == "lamboot" { print $2; exit }' "$sbat_csv")
    [[ "$cur_gen" =~ ^[1-9][0-9]*$ ]] \
        || die "could not read lamboot SBAT generation (column 2) from ${sbat_csv}"
    local new_gen=$(( cur_gen + 1 ))

    note "bumping lamboot SBAT generation: ${cur_gen} -> ${new_gen}"
    note "  This revokes every LamBoot signed at generation <= ${cur_gen}."
    note "  Only do this for a real security revocation — see packaging/SBAT-GENERATION.md."

    if (( DRY_RUN )); then
        note "[dry-run] would set column 2 of the lamboot row to ${new_gen} in ${sbat_csv}"
        return 0
    fi

    # Rewrite ONLY column 2 of the row that STARTS with `lamboot,` (the SBAT
    # header `sbat,1,...` row and the human-facing version in column 5 are left
    # untouched).
    sed -i -E "s|^(lamboot,)[0-9]+(,)|\1${new_gen}\2|" "$sbat_csv"

    local check_gen
    check_gen=$(awk -F, '$1 == "lamboot" { print $2; exit }' "$sbat_csv")
    [[ "$check_gen" == "$new_gen" ]] \
        || die "SBAT generation edit failed (got '${check_gen}', expected '${new_gen}')"
    note "packaging/sbat.csv lamboot generation is now ${new_gen}"

    note "MANDATORY follow-up — a generation bump is INERT until ALL of these are done:"
    note "  1. Re-run ./build.sh --sign (with the signing key unlocked) so dist/sbat.csv"
    note "     inherits ${new_gen} and every binary is re-embedded + re-signed at the new"
    note "     generation. A plain ./build.sh only copies the CSV; it does NOT re-sign."
    note "  2. Re-sign and re-publish every live release artifact (old generations stay"
    note "     revoked only once newer signed binaries actually ship)."
    note "  3. Coordinate the shim SBATLevel update per packaging/SBAT-GENERATION.md +"
    note "     docs/SECURE-BOOT-AND-SIGNING-STRATEGY.md before pushing the revocation."
}

# ============================================================================
# Arg parse
# ============================================================================

DRY_RUN=0
MODE=""
BUMP_VER=""
PROTOCOL_VER=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        -h|--help)          usage; exit 0 ;;
        --dry-run)          DRY_RUN=1; shift ;;
        --check)            MODE="check"; shift ;;
        --sync)             MODE="sync"; shift ;;
        --bump)             MODE="bump"; BUMP_VER="${2:?--bump requires a version}"; shift 2 ;;
        --bump=*)           MODE="bump"; BUMP_VER="${1#*=}"; shift ;;
        --bump-protocol)    MODE="bump-protocol"; PROTOCOL_VER="${2:?--bump-protocol requires N}"; shift 2 ;;
        --bump-protocol=*)  MODE="bump-protocol"; PROTOCOL_VER="${1#*=}"; shift ;;
        --bump-sbat-generation) MODE="bump-sbat-generation"; shift ;;
        *) usage >&2; exit 2 ;;
    esac
done

[[ -z "$MODE" ]] && MODE="sync"

case "$MODE" in
    check)          mode_check ;;
    sync)           mode_sync ;;
    bump)           mode_bump "$BUMP_VER" ;;
    bump-protocol)  mode_bump_protocol "$PROTOCOL_VER" ;;
    bump-sbat-generation) mode_bump_sbat_generation ;;
    *) die "unknown mode: $MODE" ;;
esac
