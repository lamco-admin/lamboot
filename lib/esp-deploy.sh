#!/bin/bash
# lamboot-dev/lib/esp-deploy.sh — canonical ESP file-layout primitives.
#
# Purpose
# -------
# Encodes the **single source of truth** for "what LamBoot files go where on
# the EFI System Partition". This is the knowledge that historically lived
# inline in tools/lamboot-install. Lifting it here lets:
#
#   * lamboot-install                  source it (live install on host)
#   * lamboot-tools' offline deploy    source the mirrored copy at
#                                      /usr/lib/lamboot-tools/lib/esp-deploy.sh
#
# Mirror flow: lamboot-tools-dev/publish/mirror-from-lamboot-dev.sh copies
# this file at release-build time and records its sha256 in
# MIRROR-CHECKSUMS.txt for drift detection. Per
# docs/specs/SPEC-LAMBOOT-TOOLKIT-V1.md §14.2 canonical-source-map.
#
# Design rules (intentionally narrow)
# -----------------------------------
# 1. Pure functions: no global mutation, no logging-framework calls
#    (no ok/warn/die/detail/run — those are caller flair).
# 2. Stdout emits installed relative paths, one per line. Caller pipes
#    those into esp_manifest_write to produce .install-manifest.
# 3. Stderr emits "lamboot-esp-deploy: warn: …" for non-fatal issues.
# 4. Return code: 0 success, 1 unrecoverable, 2 partial (some items failed).
# 5. No dry-run knob in the lib — the caller decides whether to invoke.
# 6. No reliance on bash 4.x associative arrays beyond what the caller
#    already requires; functions accept positional args only.
#
# Conventions
# -----------
# Source layout (dist/EFI/LamBoot/):
#   lambootx64.efi              (unsigned canonical)
#   lambootx64-signed.efi       (signed variant)
#   modules/<name>.efi          (unsigned canonical)
#   modules/<name>-signed.efi   (signed variant)
#   modules/manifest.toml       (module metadata)
#   drivers/<name>.efi          (unsigned canonical)
#   drivers/<name>-signed.efi   (signed variant)
#   drivers/LICENSE-GPL-2.0.txt (must ship alongside ext{2,3,4} drivers)
#   policy.toml                 (config defaults)
#
# Destination layout (ESP under /EFI/LamBoot/):
#   ALWAYS the canonical (unsigned) name; the firmware loads what the
#   Boot#### entry points at, and lamboot-repair --nvram only knows
#   the canonical names. Preserving the -signed suffix on the ESP is
#   exactly the bug that broke VM 123 (NVRAM Boot0007 pointed at
#   lambootx64.efi but file was at lambootx64-signed.efi → Not Found).
#
# ── Constants (readonly, sourced once) ───────────────────────────────────

# Re-source guard: this lib may be sourced multiple times (e.g., lamboot-
# install + a sub-script). Don't re-readonly on second source.
if [[ -z "${_LAMBOOT_ESP_DEPLOY_SOURCED:-}" ]]; then
    # These constants are part of the lib's public API; consumers
    # (lamboot-install, lamboot-tools' offline deploy) reference them
    # by name. shellcheck doesn't see those external uses.
    # shellcheck disable=SC2034  # public API constant
    readonly LAMBOOT_ESP_LIB_VERSION="1.0.0"
    readonly LAMBOOT_ESP_LIB_DIR="EFI/LamBoot"
    readonly LAMBOOT_ESP_MANIFEST_REL="EFI/LamBoot/.install-manifest"
    # shellcheck disable=SC2034  # public API constant
    readonly LAMBOOT_ESP_MANIFEST_FILE=".install-manifest"
    _LAMBOOT_ESP_DEPLOY_SOURCED=1
fi

# ── Logging (minimal, stderr-only, prefixed) ─────────────────────────────

_esp_warn() {
    printf 'lamboot-esp-deploy: warn: %s\n' "$1" >&2
}

_esp_err() {
    printf 'lamboot-esp-deploy: error: %s\n' "$1" >&2
}

# ── Architecture-specific naming ─────────────────────────────────────────

# esp_efi_binary_name ARCH
#   echoes the canonical ESP filename for the bootloader binary
esp_efi_binary_name() {
    case "$1" in
        x86_64)  echo "lambootx64.efi" ;;
        aarch64) echo "lambootaa64.efi" ;;
        *)       _esp_err "unsupported arch: $1"; return 1 ;;
    esac
}

# esp_efi_source_filename ARCH IS_SIGNED
#   echoes the source filename in dist/EFI/LamBoot/ — bare name when
#   IS_SIGNED=0, "-signed.efi" variant when IS_SIGNED=1
esp_efi_source_filename() {
    local arch="$1" is_signed="$2"
    if [[ "$is_signed" == "1" ]]; then
        case "$arch" in
            x86_64)  echo "lambootx64-signed.efi" ;;
            aarch64) echo "lambootaa64-signed.efi" ;;
            *)       _esp_err "unsupported arch: $arch"; return 1 ;;
        esac
    else
        esp_efi_binary_name "$arch"
    fi
}

# esp_module_canonical_name SRC_FILENAME
#   Strips the -signed suffix to yield the canonical destination name.
#   diag-shell-signed.efi → diag-shell.efi
#   diag-shell.efi        → diag-shell.efi
esp_module_canonical_name() {
    local name="$1"
    case "$name" in
        *-signed.efi) echo "${name%-signed.efi}.efi" ;;
        *.efi)        echo "$name" ;;
        *)            _esp_err "not a .efi module name: $name"; return 1 ;;
    esac
}

# esp_module_source_for SRC_DIR CANONICAL_NAME IS_SIGNED
#   Resolves the source file path in dist/EFI/LamBoot/modules/.
#   When IS_SIGNED=1, prefers the -signed variant; falls back to canonical
#   with a warning if no signed variant exists. Returns 1 if neither exists.
esp_module_source_for() {
    local src_dir="$1" canonical="$2" is_signed="$3"
    local canonical_path="${src_dir}/EFI/LamBoot/modules/${canonical}"
    local signed_path="${src_dir}/EFI/LamBoot/modules/${canonical%.efi}-signed.efi"

    if [[ "$is_signed" == "1" ]]; then
        if [[ -f "$signed_path" ]]; then
            echo "$signed_path"
            return 0
        fi
        _esp_warn "signed module not found, falling back to unsigned: ${canonical}"
    fi
    if [[ -f "$canonical_path" ]]; then
        echo "$canonical_path"
        return 0
    fi
    return 1
}

# Same shape, for drivers.
esp_driver_source_for() {
    local src_dir="$1" canonical="$2" is_signed="$3"
    local canonical_path="${src_dir}/EFI/LamBoot/drivers/${canonical}"
    local signed_path="${src_dir}/EFI/LamBoot/drivers/${canonical%.efi}-signed.efi"

    if [[ "$is_signed" == "1" ]]; then
        if [[ -f "$signed_path" ]]; then
            echo "$signed_path"
            return 0
        fi
        _esp_warn "signed driver not found, falling back to unsigned: ${canonical}"
    fi
    if [[ -f "$canonical_path" ]]; then
        echo "$canonical_path"
        return 0
    fi
    return 1
}

# ── File primitives ──────────────────────────────────────────────────────

esp_atomic_copy() {
    local src="$1" dst="$2"
    local dst_dir
    dst_dir=$(dirname "$dst")
    mkdir -p "$dst_dir" || return 1
    local tmp="${dst}.lamboot-esp-deploy.$$"
    cp -- "$src" "$tmp" || { rm -f "$tmp"; return 1; }
    mv -f -- "$tmp" "$dst" || { rm -f "$tmp"; return 1; }
}

esp_file_sha256() {
    sha256sum -- "$1" 2>/dev/null | cut -d' ' -f1
}

# esp_needs_update SRC DST → 0 if dst missing or hashes differ
esp_needs_update() {
    local src="$1" dst="$2"
    [[ -f "$dst" ]] || return 0
    local sh_src sh_dst
    sh_src=$(esp_file_sha256 "$src") || return 0
    sh_dst=$(esp_file_sha256 "$dst") || return 0
    [[ "$sh_src" != "$sh_dst" ]]
}

# ── Deploy operations ────────────────────────────────────────────────────
# Each function emits installed relative paths on stdout, one per line.

# esp_deploy_binary ESP SRC_DIR ARCH IS_SIGNED
esp_deploy_binary() {
    local esp="$1" src_dir="$2" arch="$3" is_signed="$4"
    local src_name dst_name
    src_name=$(esp_efi_source_filename "$arch" "$is_signed") || return 1
    dst_name=$(esp_efi_binary_name "$arch") || return 1
    local src="${src_dir}/EFI/LamBoot/${src_name}"
    local rel="${LAMBOOT_ESP_LIB_DIR}/${dst_name}"
    local dst="${esp}/${rel}"

    if [[ ! -f "$src" ]]; then
        _esp_err "binary source not found: ${src}"
        return 1
    fi
    if esp_needs_update "$src" "$dst"; then
        esp_atomic_copy "$src" "$dst" || { _esp_err "failed to install ${dst_name}"; return 1; }
    fi
    echo "$rel"
}

# esp_deploy_modules ESP SRC_DIR IS_SIGNED [INCLUDE_MANIFEST_TOML=1]
# Iterates dist/EFI/LamBoot/modules/*.efi (canonical names only — we
# explicitly skip *-signed.efi files because they're handled via the
# canonical name's signed variant resolver). Emits one relative path
# per installed file.
esp_deploy_modules() {
    local esp="$1" src_dir="$2" is_signed="$3" include_manifest="${4:-1}"
    local mods_src="${src_dir}/EFI/LamBoot/modules"
    [[ -d "$mods_src" ]] || return 0

    local errs=0

    if [[ "$include_manifest" == "1" ]]; then
        local mf_src="${mods_src}/manifest.toml"
        if [[ -f "$mf_src" ]]; then
            local mf_rel="${LAMBOOT_ESP_LIB_DIR}/modules/manifest.toml"
            local mf_dst="${esp}/${mf_rel}"
            if esp_atomic_copy "$mf_src" "$mf_dst"; then
                echo "$mf_rel"
            else
                _esp_warn "failed to install module manifest"
                errs=$((errs + 1))
            fi
        fi
    fi

    local mod
    for mod in "${mods_src}"/*.efi; do
        [[ -f "$mod" ]] || continue
        local name
        name=$(basename "$mod")
        # Skip -signed.efi files; they're handled via the canonical resolver
        case "$name" in *-signed.efi) continue ;; esac

        local src
        if ! src=$(esp_module_source_for "$src_dir" "$name" "$is_signed"); then
            _esp_warn "no source for module ${name} (canonical or signed)"
            errs=$((errs + 1))
            continue
        fi
        local rel="${LAMBOOT_ESP_LIB_DIR}/modules/${name}"
        local dst="${esp}/${rel}"
        if esp_needs_update "$src" "$dst"; then
            if ! esp_atomic_copy "$src" "$dst"; then
                _esp_warn "failed to install module ${name}"
                errs=$((errs + 1))
                continue
            fi
        fi
        echo "$rel"
    done
    [[ $errs -eq 0 ]]
}

# esp_deploy_drivers ESP SRC_DIR ARCH IS_SIGNED FS_LIST
# FS_LIST: newline-separated driver basenames (e.g. "ext4_x64.efi\nntfs_x64.efi").
# Caller decides which drivers to deploy (NEED_FS_DRIVER analysis lives
# in install-tool, not here). Also installs LICENSE-GPL-2.0.txt when present.
esp_deploy_drivers() {
    local esp="$1" src_dir="$2" arch="$3" is_signed="$4"
    shift 4
    local fs_list="$*"
    [[ -n "$fs_list" ]] || return 0

    local errs=0
    local drv
    while IFS= read -r drv; do
        [[ -n "$drv" ]] || continue
        local src
        if ! src=$(esp_driver_source_for "$src_dir" "$drv" "$is_signed"); then
            _esp_warn "driver source not found: ${drv}"
            errs=$((errs + 1))
            continue
        fi
        local rel="${LAMBOOT_ESP_LIB_DIR}/drivers/${drv}"
        local dst="${esp}/${rel}"
        if esp_needs_update "$src" "$dst"; then
            if ! esp_atomic_copy "$src" "$dst"; then
                _esp_warn "failed to install driver ${drv}"
                errs=$((errs + 1))
                continue
            fi
        fi
        echo "$rel"
    done <<< "$fs_list"

    # GPL license file ships alongside drivers; manifest-track it so
    # --remove cleans up (without this, rmdir on drivers/ silently fails).
    local lic_src="${src_dir}/EFI/LamBoot/drivers/LICENSE-GPL-2.0.txt"
    if [[ -f "$lic_src" ]]; then
        local lic_rel="${LAMBOOT_ESP_LIB_DIR}/drivers/LICENSE-GPL-2.0.txt"
        local lic_dst="${esp}/${lic_rel}"
        esp_atomic_copy "$lic_src" "$lic_dst" 2>/dev/null && echo "$lic_rel"
    fi

    [[ $errs -eq 0 ]]
}

# esp_deploy_policy ESP SRC_DIR [PRESERVE_EXISTING=1]
# When PRESERVE_EXISTING=1 and policy.toml already exists with different
# content, writes a sibling policy.toml.new instead of overwriting (so the
# admin's edits survive). When 0, overwrites unconditionally.
# Always emits the canonical relative path of the LIVE policy.toml so the
# caller can manifest-track it.
esp_deploy_policy() {
    local esp="$1" src_dir="$2" preserve="${3:-1}"
    local src="${src_dir}/EFI/LamBoot/policy.toml"
    [[ -f "$src" ]] || return 0
    local rel="${LAMBOOT_ESP_LIB_DIR}/policy.toml"
    local dst="${esp}/${rel}"

    if [[ -f "$dst" ]] && [[ "$preserve" == "1" ]]; then
        if esp_needs_update "$src" "$dst"; then
            esp_atomic_copy "$src" "${dst}.new" 2>/dev/null || true
        fi
    else
        esp_atomic_copy "$src" "$dst" || { _esp_warn "failed to install policy.toml"; return 1; }
    fi
    echo "$rel"
}

# ── Manifest format ──────────────────────────────────────────────────────

# esp_manifest_write ESP VERSION ARCH DISTRO < relative-paths-on-stdin
# Hashes each $ESP/$rel and writes EFI/LamBoot/.install-manifest.
esp_manifest_write() {
    local esp="$1" version="$2" arch="$3" distro="$4"
    local dst="${esp}/${LAMBOOT_ESP_MANIFEST_REL}"
    local tmp="${dst}.lamboot-esp-deploy.$$"
    local ts
    ts=$(date -u '+%Y-%m-%dT%H:%M:%S')

    {
        echo "# LamBoot Install Manifest"
        echo "# Generated: ${ts}"
        echo "# Version: ${version}"
        echo "# Arch: ${arch}"
        echo "# Distro: ${distro}"
        local rel
        while IFS= read -r rel; do
            [[ -n "$rel" ]] || continue
            local hash
            hash=$(esp_file_sha256 "${esp}/${rel}")
            [[ -n "$hash" ]] || { _esp_warn "manifest: cannot hash ${rel}"; continue; }
            echo "sha256:${hash}  ${rel}"
        done
    } > "$tmp" || { rm -f "$tmp"; _esp_err "failed to write manifest"; return 1; }

    mv -f "$tmp" "$dst" || { rm -f "$tmp"; _esp_err "failed to finalize manifest"; return 1; }
}

# esp_manifest_parse ESP
# Echoes one line per entry: "HASH<TAB>RELATIVE_PATH"
# First line is metadata: "VERSION<TAB>ARCH<TAB>DISTRO" (any field may be empty).
# Returns 1 if no manifest present.
esp_manifest_parse() {
    local esp="$1"
    local mf="${esp}/${LAMBOOT_ESP_MANIFEST_REL}"
    [[ -f "$mf" ]] || return 1

    local version="" arch="" distro=""
    local entries=()
    local line
    while IFS= read -r line; do
        case "$line" in
            "# Version: "*)  version="${line#\# Version: }" ;;
            "# Arch: "*)     arch="${line#\# Arch: }" ;;
            "# Distro: "*)   distro="${line#\# Distro: }" ;;
            "#"*|"")         continue ;;
            sha256:*)
                local hash path
                hash="${line%%  *}"
                hash="${hash#sha256:}"
                path="${line#*  }"
                entries+=("${hash}"$'\t'"${path}")
                ;;
        esac
    done < "$mf"

    printf '%s\t%s\t%s\n' "$version" "$arch" "$distro"
    local e
    for e in "${entries[@]}"; do
        echo "$e"
    done
}

# esp_remove_by_manifest ESP [FORCE=0] [KEEP_BLS=0]
# Reads the manifest, removes each listed file whose current hash matches
# the recorded hash (or unconditionally when FORCE=1). Skips paths under
# loader/entries/ when KEEP_BLS=1. Echoes removed paths on stdout.
# Returns 1 if no manifest present.
esp_remove_by_manifest() {
    local esp="$1" force="${2:-0}" keep_bls="${3:-0}"
    local mf="${esp}/${LAMBOOT_ESP_MANIFEST_REL}"
    [[ -f "$mf" ]] || return 1

    local first=1
    local hash path full current
    while IFS=$'\t' read -r hash path _; do
        if (( first )); then first=0; continue; fi
        full="${esp}/${path}"
        [[ -f "$full" ]] || continue
        if [[ "$keep_bls" == "1" ]] && [[ "$path" == loader/entries/* ]]; then
            continue
        fi
        current=$(esp_file_sha256 "$full")
        if [[ "$current" == "$hash" ]] || [[ "$force" == "1" ]]; then
            rm -f "$full" || { _esp_warn "failed to remove ${path}"; continue; }
            echo "$path"
        else
            _esp_warn "skipping modified file: ${path}"
        fi
    done < <(esp_manifest_parse "$esp")

    # Manifest itself before dir cleanup — otherwise rmdir
    # ${LAMBOOT_ESP_LIB_DIR} fails because .install-manifest still lives
    # inside it.
    rm -f "$mf" 2>/dev/null || true

    # Best-effort empty-dir cleanup, deepest-first
    local d
    for d in modules drivers; do
        rmdir "${esp}/${LAMBOOT_ESP_LIB_DIR}/${d}" 2>/dev/null || true
    done
    rmdir "${esp}/${LAMBOOT_ESP_LIB_DIR}" 2>/dev/null || true
}
