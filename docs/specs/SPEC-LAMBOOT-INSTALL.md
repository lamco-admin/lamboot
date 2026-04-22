# SPEC-LAMBOOT-INSTALL: Installation Script Specification

**Version:** 1.0
**Date:** 2026-04-03
**Status:** Implementation-Ready
**Target:** `tools/lamboot-install`

---

## 1. Overview

`lamboot-install` is the single-command installer for the LamBoot UEFI bootloader.
It detects the system environment, copies files to the ESP, generates BLS entries
where needed, creates a UEFI boot entry, and installs systemd integration. It is
idempotent, non-destructive, and distro-agnostic.

**Scope boundary:** This script handles INITIAL installation and removal. Ongoing
kernel updates are handled by `dist/kernel-install/90-lamboot.install` and must
not be duplicated here.

### 1.1 Constraints

- Single bash file, `#!/bin/bash`, POSIX-compatible where possible
- Zero dependencies beyond coreutils + efibootmgr
- No Python, Perl, jq, or non-standard utilities
- Must run as root (uid 0)
- Target: any Linux distribution with UEFI firmware

### 1.2 Artifact Locations

| Source (repo)                                | Destination (ESP)                          |
|----------------------------------------------|--------------------------------------------|
| `dist/EFI/LamBoot/lambootx64.efi`           | `$ESP/EFI/LamBoot/lambootx64.efi`         |
| `dist/EFI/LamBoot/lambootaa64.efi`          | `$ESP/EFI/LamBoot/lambootaa64.efi`        |
| `dist/EFI/LamBoot/drivers/*.efi`            | `$ESP/EFI/LamBoot/drivers/*.efi`          |
| `dist/EFI/LamBoot/drivers/aarch64/*.efi`    | `$ESP/EFI/LamBoot/drivers/aarch64/*.efi`  |
| `dist/EFI/LamBoot/modules/manifest.toml`    | `$ESP/EFI/LamBoot/modules/manifest.toml`  |
| `dist/EFI/LamBoot/policy.toml`              | `$ESP/EFI/LamBoot/policy.toml` (no clobber) |
| `dist/kernel-install/90-lamboot.install`     | `/usr/lib/kernel/install.d/90-lamboot.install` |
| `dist/systemd/lamboot-mark-success.service`  | `/usr/lib/systemd/system/lamboot-mark-success.service` |

---

## 2. CLI Interface

```
lamboot-install [OPTIONS]

Options:
  --esp PATH        Override ESP mount point
  --no-efi-entry    Skip UEFI boot entry creation (file copy only)
  --set-default     Set LamBoot as first boot option in UEFI
  --fallback        Also install as \EFI\BOOT\BOOTX64.EFI (or BOOTAA64.EFI)
  --with-drivers    Install filesystem drivers (auto-detected when omitted)
  --with-modules    Install diagnostic modules
  --remove          Remove LamBoot installation using manifest
  --update          Update existing installation, preserve config
  --dry-run         Print actions without executing
  --force           Skip safety checks (ESP validation, space, etc.)
  --no-bls          Skip BLS entry generation
  --keep-entries    With --remove: do not delete generated BLS entries
  --quiet           Suppress informational output (errors still printed)
  --verbose         Print detailed debug output
  --version         Print version and exit
  --help            Print usage and exit
```

### 2.1 Exit Codes

| Code | Constant      | Meaning                                  |
|------|---------------|------------------------------------------|
| 0    | `EXIT_OK`     | All operations succeeded                 |
| 1    | `EXIT_ERROR`  | Fatal error, installation aborted        |
| 2    | `EXIT_PARTIAL`| Core install succeeded, non-critical step failed |
| 3    | `EXIT_NOOP`   | Nothing to do (already up-to-date)       |

### 2.2 Option Validation

Mutually exclusive sets enforced at parse time:

- `--remove` excludes: `--set-default`, `--fallback`, `--with-drivers`, `--with-modules`, `--update`, `--no-bls`
- `--update` excludes: `--remove`
- `--quiet` excludes: `--verbose`

Invalid combinations call `die` with a message naming both conflicting flags.

---

## 3. Constants

```bash
readonly LAMBOOT_VERSION="0.2.0"
readonly LAMBOOT_LABEL="LamBoot"

# ESP paths (backslash form for efibootmgr, forward-slash for filesystem)
readonly EFI_DIR="EFI/LamBoot"
readonly EFI_LOADER_PATH="\\EFI\\LamBoot\\lambootx64.efi"
readonly EFI_LOADER_PATH_AA64="\\EFI\\LamBoot\\lambootaa64.efi"
readonly FALLBACK_DIR="EFI/BOOT"
readonly FALLBACK_NAME_X64="BOOTX64.EFI"
readonly FALLBACK_NAME_AA64="BOOTAA64.EFI"

# Manifest
readonly MANIFEST_FILE=".install-manifest"
readonly MANIFEST_PATH="EFI/LamBoot/${MANIFEST_FILE}"

# ESP partition type GUID (case-insensitive match)
readonly ESP_PARTTYPE_GUID="c12a7328-f81f-11d2-ba4b-00a0c93ec93b"

# Minimum ESP free space in KiB
readonly MIN_ESP_SPACE_KIB=2048

# Systemd paths
readonly SYSTEMD_UNIT_DIR="/usr/lib/systemd/system"
readonly KERNEL_INSTALL_DIR="/usr/lib/kernel/install.d"

# BLS
readonly BLS_DIR="loader/entries"
```

---

## 4. Global State Variables

```bash
# Set during option parsing
OPT_ESP=""            # --esp value, empty = auto-detect
OPT_NO_EFI_ENTRY=0   # --no-efi-entry
OPT_SET_DEFAULT=0     # --set-default
OPT_FALLBACK=0        # --fallback
OPT_WITH_DRIVERS=-1   # --with-drivers: -1=auto, 0=no, 1=yes
OPT_WITH_MODULES=0    # --with-modules
OPT_REMOVE=0          # --remove
OPT_UPDATE=0          # --update
OPT_DRY_RUN=0         # --dry-run
OPT_FORCE=0           # --force
OPT_NO_BLS=0          # --no-bls
OPT_KEEP_ENTRIES=0    # --keep-entries
OPT_QUIET=0           # --quiet
OPT_VERBOSE=0         # --verbose

# Set during Phase 1
ESP=""                # Validated ESP mount point (absolute path)
ARCH=""               # "x86_64" or "aarch64"
DISTRO_ID=""          # /etc/os-release ID field (e.g., "fedora", "debian")
DISTRO_NAME=""        # /etc/os-release PRETTY_NAME
DISTRO_VERSION=""     # /etc/os-release VERSION_ID
HAS_EXISTING=0        # 1 if prior LamBoot installation found on ESP
SECURE_BOOT=0         # 1 if Secure Boot enabled
IS_CHROOT=0           # 1 if running inside chroot

# Set during Phase 1 (ESP device info for efibootmgr)
ESP_DISK=""           # e.g., "/dev/sda"
ESP_PARTNUM=""        # e.g., "1"

# Set during Phase 2
BOOT_FSTYPE=""        # Filesystem type of /boot (e.g., "ext4", "vfat")
NEED_FS_DRIVER=0      # 1 if /boot is on non-FAT and not on ESP

# Accumulator for manifest
declare -a MANIFEST_ENTRIES=()

# Track partial failures
PARTIAL_FAILURE=0
```

---

## 5. Utility Functions

### 5.1 Output

```bash
# msg(text) — informational output, suppressed by --quiet
msg() {
    (( OPT_QUIET )) || printf '%s\n' "$1"
}

# detail(text) — verbose output, only with --verbose
detail() {
    (( OPT_VERBOSE )) && printf '  %s\n' "$1"
}

# warn(text) — warning to stderr, never suppressed
warn() {
    printf 'WARNING: %s\n' "$1" >&2
}

# die(text) — fatal error, exit 1
die() {
    printf 'ERROR: %s\n' "$1" >&2
    exit 1
}

# ok(text) — success checkmark line
ok() {
    (( OPT_QUIET )) || printf '  \xe2\x9c\x93 %s\n' "$1"
}

# fail(text) — failure cross line
fail() {
    printf '  \xe2\x9c\x97 %s\n' "$1" >&2
}
```

### 5.2 Dry-Run Wrapper

```bash
# run(description, command...) — execute or print depending on --dry-run
# Returns: command exit code, or 0 in dry-run mode
run() {
    local desc="$1"; shift
    if (( OPT_DRY_RUN )); then
        msg "  [dry-run] ${desc}"
        return 0
    fi
    detail "exec: $*"
    "$@"
}
```

### 5.3 Atomic Copy

```bash
# atomic_copy(src, dst) — copy via temp file then mv (crash-safe)
# Creates parent directories as needed.
# Returns: 0 on success, 1 on failure
atomic_copy() {
    local src="$1" dst="$2"
    local dst_dir
    dst_dir=$(dirname "$dst")

    run "mkdir -p ${dst_dir}" mkdir -p "$dst_dir" || return 1

    local tmp="${dst}.lamboot-tmp.$$"
    run "copy ${src} -> ${dst}" cp -- "$src" "$tmp" || { rm -f "$tmp"; return 1; }
    run "atomic rename ${tmp} -> ${dst}" mv -f -- "$tmp" "$dst" || { rm -f "$tmp"; return 1; }
    return 0
}
```

### 5.4 Checksum

```bash
# file_sha256(path) — print sha256 hex digest of file
# Returns: 0, prints digest to stdout. 1 if file unreadable.
file_sha256() {
    sha256sum -- "$1" 2>/dev/null | cut -d' ' -f1
}
```

### 5.5 Manifest Accumulator

```bash
# manifest_add(esp_relative_path) — compute sha256 and append to MANIFEST_ENTRIES
# The path is relative to ESP root (e.g., "EFI/LamBoot/lambootx64.efi").
manifest_add() {
    local rel="$1"
    local hash
    hash=$(file_sha256 "${ESP}/${rel}")
    MANIFEST_ENTRIES+=("sha256:${hash}  ${rel}")
}
```

---

## 6. Phase 1: Environment Detection

Entry point: `phase1_detect_environment()`
Returns: 0 on success, calls `die` on fatal errors.

### 6.1 Root Check

```bash
# At script top, before any phase
(( EUID == 0 )) || die "This script must be run as root."
```

### 6.2 Architecture Detection

```bash
detect_arch() {
    case "$(uname -m)" in
        x86_64)  ARCH="x86_64" ;;
        aarch64) ARCH="aarch64" ;;
        *)       die "Unsupported architecture: $(uname -m)" ;;
    esac
}
```

### 6.3 EFI Binary Selection

```bash
# efi_binary() — return filename of the correct EFI binary for this arch
efi_binary() {
    case "$ARCH" in
        x86_64)  echo "lambootx64.efi" ;;
        aarch64) echo "lambootaa64.efi" ;;
    esac
}

# efi_loader_path() — return backslash EFI path for efibootmgr
efi_loader_path() {
    case "$ARCH" in
        x86_64)  echo "$EFI_LOADER_PATH" ;;
        aarch64) echo "$EFI_LOADER_PATH_AA64" ;;
    esac
}

# fallback_name() — return fallback filename for this arch
fallback_name() {
    case "$ARCH" in
        x86_64)  echo "$FALLBACK_NAME_X64" ;;
        aarch64) echo "$FALLBACK_NAME_AA64" ;;
    esac
}
```

### 6.4 Source Directory Resolution

```bash
# find_source_dir() — locate the dist/ tree relative to this script
# Returns: 0 and prints path, or 1 if not found
find_source_dir() {
    local script_dir
    script_dir=$(cd "$(dirname "$0")" && pwd)

    # tools/lamboot-install -> repo root is ..
    local repo_root="${script_dir}/.."
    if [ -f "${repo_root}/dist/EFI/LamBoot/$(efi_binary)" ]; then
        echo "${repo_root}/dist"
        return 0
    fi

    # Fallback: check /usr/share/lamboot
    if [ -f "/usr/share/lamboot/EFI/LamBoot/$(efi_binary)" ]; then
        echo "/usr/share/lamboot"
        return 0
    fi

    return 1
}
```

Global set in `phase1`:
```bash
SRC_DIR=""  # Set by find_source_dir
```

### 6.5 Chroot Detection

```bash
detect_chroot() {
    if [ ! -d /sys/firmware/efi ]; then
        if [ -e /proc/1/root ] && [ "$(stat -c %d:%i /)" != "$(stat -c %d:%i /proc/1/root 2>/dev/null)" ]; then
            IS_CHROOT=1
        fi
    fi
}
```

### 6.6 Secure Boot Detection

```bash
detect_secure_boot() {
    local sb_var="/sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c"
    if [ -f "$sb_var" ]; then
        # Last byte: 0=disabled, 1=enabled
        local val
        val=$(od -An -t u1 -j4 -N1 "$sb_var" 2>/dev/null | tr -d ' ')
        (( val == 1 )) && SECURE_BOOT=1
    fi
}
```

### 6.7 Distro Detection

```bash
detect_distro() {
    if [ -r /etc/os-release ]; then
        . /etc/os-release
        DISTRO_ID="${ID:-unknown}"
        DISTRO_NAME="${PRETTY_NAME:-Linux}"
        DISTRO_VERSION="${VERSION_ID:-}"
    else
        DISTRO_ID="unknown"
        DISTRO_NAME="Linux"
        DISTRO_VERSION=""
    fi
}
```

### 6.8 ESP Detection (5-Method Cascade)

```bash
# is_vfat(mountpoint) — true if filesystem is vfat
is_vfat() {
    local fstype
    fstype=$(findmnt -n -o FSTYPE "$1" 2>/dev/null)
    [ "$fstype" = "vfat" ]
}

# is_esp_partition(mountpoint) — true if underlying partition has ESP type GUID
is_esp_partition() {
    local dev
    dev=$(findmnt -n -o SOURCE "$1" 2>/dev/null)
    [ -n "$dev" ] || return 1
    local parttype
    parttype=$(lsblk -nro PARTTYPE "$dev" 2>/dev/null)
    echo "$parttype" | grep -qi "$ESP_PARTTYPE_GUID"
}

# validate_esp(mountpoint) — comprehensive ESP validation
# Returns: 0=valid, 1=invalid (prints reason to stderr)
validate_esp() {
    local mp="$1"

    mountpoint -q "$mp" 2>/dev/null || { detail "not a mountpoint: ${mp}"; return 1; }
    is_vfat "$mp"                   || { detail "not vfat: ${mp}"; return 1; }

    if ! (( OPT_FORCE )); then
        is_esp_partition "$mp"      || { detail "not ESP partition type: ${mp}"; return 1; }
    fi

    # Writable test
    if ! (( OPT_DRY_RUN )); then
        touch "${mp}/.lamboot-write-test" 2>/dev/null && rm -f "${mp}/.lamboot-write-test" \
            || { detail "not writable: ${mp}"; return 1; }
    fi

    # Space check
    local avail
    avail=$(df -k --output=avail "$mp" 2>/dev/null | tail -1 | tr -d ' ')
    if [ -n "$avail" ] && (( avail < MIN_ESP_SPACE_KIB )) && ! (( OPT_FORCE )); then
        warn "ESP has only ${avail} KiB free (need ${MIN_ESP_SPACE_KIB} KiB)"
        return 1
    fi

    return 0
}

# find_esp() — detect ESP mount point via 5-method cascade
# Sets: ESP, ESP_DISK, ESP_PARTNUM
# Returns: 0=found, 1=not found (calls die)
find_esp() {
    # Method 0: User override
    if [ -n "$OPT_ESP" ]; then
        if validate_esp "$OPT_ESP"; then
            ESP="$OPT_ESP"
        else
            die "Specified ESP path '${OPT_ESP}' is not a valid EFI System Partition."
        fi
        _resolve_esp_device
        return 0
    fi

    # Method 1: Standard mount points (bootctl order)
    local mp
    for mp in /efi /boot/efi /boot; do
        if mountpoint -q "$mp" 2>/dev/null && validate_esp "$mp"; then
            ESP="$mp"
            detail "ESP found via standard mount point: ${ESP}"
            _resolve_esp_device
            return 0
        fi
    done

    # Method 2: findmnt by filesystem type
    mp=$(findmnt -n -o TARGET -t vfat --first-only 2>/dev/null)
    if [ -n "$mp" ] && validate_esp "$mp"; then
        ESP="$mp"
        detail "ESP found via findmnt: ${ESP}"
        _resolve_esp_device
        return 0
    fi

    # Method 3: /etc/fstab for unmounted ESPs
    if [ -r /etc/fstab ]; then
        mp=$(awk '$3 == "vfat" && ($2 ~ /boot|efi/) { print $2; exit }' /etc/fstab)
        if [ -n "$mp" ] && ! mountpoint -q "$mp" 2>/dev/null; then
            detail "Attempting to mount ESP from fstab: ${mp}"
            mount "$mp" 2>/dev/null
            if validate_esp "$mp"; then
                ESP="$mp"
                detail "ESP found via fstab: ${ESP}"
                _resolve_esp_device
                return 0
            fi
        fi
    fi

    # Method 4: Block device scan by partition type GUID
    local esp_part
    esp_part=$(lsblk -nrpo NAME,PARTTYPE 2>/dev/null \
        | grep -i "$ESP_PARTTYPE_GUID" \
        | awk '{print $1}' | head -1)
    if [ -n "$esp_part" ]; then
        # Check if already mounted
        mp=$(findmnt -n -o TARGET "$esp_part" 2>/dev/null)
        if [ -z "$mp" ]; then
            mp=$(mktemp -d /tmp/lamboot-esp-XXXXXX)
            detail "Mounting ESP from block scan: ${esp_part} -> ${mp}"
            mount "$esp_part" "$mp" 2>/dev/null || { rmdir "$mp"; }
        fi
        if [ -n "$mp" ] && validate_esp "$mp"; then
            ESP="$mp"
            detail "ESP found via block scan: ${ESP}"
            _resolve_esp_device
            return 0
        fi
    fi

    die "EFI System Partition not found. Tried: standard mounts, findmnt, fstab, block scan.
  Mount your ESP and retry, or specify it with --esp PATH."
}

# _resolve_esp_device() — populate ESP_DISK and ESP_PARTNUM from ESP
_resolve_esp_device() {
    local dev
    dev=$(findmnt -n -o SOURCE "$ESP" 2>/dev/null)
    [ -n "$dev" ] || return 0

    ESP_DISK="/dev/$(lsblk -nro PKNAME "$dev" 2>/dev/null)"
    ESP_PARTNUM=$(lsblk -nro PARTN "$dev" 2>/dev/null)

    # Fallback: parse partition number from device name (e.g., /dev/sda1 -> 1)
    if [ -z "$ESP_PARTNUM" ]; then
        ESP_PARTNUM=$(echo "$dev" | sed 's/.*[^0-9]\([0-9]\+\)$/\1/')
    fi
}
```

### 6.9 Existing Installation Detection

```bash
detect_existing() {
    if [ -f "${ESP}/${EFI_DIR}/$(efi_binary)" ]; then
        HAS_EXISTING=1
        detail "Existing LamBoot installation found on ESP"
    fi
}
```

### 6.10 Phase 1 Orchestrator

```bash
phase1_detect_environment() {
    msg "Phase 1: Detecting environment..."

    detect_arch
    detail "Architecture: ${ARCH}"

    SRC_DIR=$(find_source_dir) || die "Cannot find LamBoot distribution files.
  Run from the repository root or install to /usr/share/lamboot."

    local bin="${SRC_DIR}/EFI/LamBoot/$(efi_binary)"
    [ -f "$bin" ] || die "EFI binary not found: ${bin}
  Run ./build.sh first."

    detect_chroot
    (( IS_CHROOT )) && detail "Chroot environment detected"

    detect_distro
    detail "Distro: ${DISTRO_NAME} (${DISTRO_ID})"

    find_esp
    ok "ESP: ${ESP} ($(df -h --output=avail "${ESP}" 2>/dev/null | tail -1 | tr -d ' ') free)"

    detect_existing
    detect_secure_boot
    (( SECURE_BOOT )) && detail "Secure Boot: enabled"

    # Warn on --update without existing installation
    if (( OPT_UPDATE )) && ! (( HAS_EXISTING )); then
        warn "No existing LamBoot installation found; performing fresh install."
    fi
}
```

---

## 7. Phase 2: Filesystem Driver Assessment

Entry point: `phase2_assess_drivers()`

### 7.1 Logic

```
IF /boot is a separate mountpoint from ESP:
    BOOT_FSTYPE = filesystem type of /boot
    IF BOOT_FSTYPE is not "vfat":
        NEED_FS_DRIVER = 1
ELSE:
    NEED_FS_DRIVER = 0
```

When `OPT_WITH_DRIVERS == -1` (auto), the script installs drivers only when
`NEED_FS_DRIVER == 1`. When `OPT_WITH_DRIVERS == 1` (explicit), drivers are always
installed. When `OPT_WITH_DRIVERS == 0`, drivers are never installed (user suppressed
auto-detection externally via `--with-drivers=no`; this is an undocumented escape hatch
but the variable state is not currently settable from CLI; auto and explicit are the
two reachable states).

### 7.2 Driver File Selection

```bash
# driver_files_for_arch() — list driver filenames needed for current arch + fstype
# Prints one filename per line. Caller resolves against SRC_DIR.
driver_files_for_arch() {
    local base_dir="${SRC_DIR}/EFI/LamBoot/drivers"

    case "$ARCH" in
        x86_64)
            case "$BOOT_FSTYPE" in
                ext2) echo "ext2_x64.efi" ;;
                ext3|ext4) echo "ext4_x64.efi" ;;
                btrfs) echo "btrfs_x64.efi" ;;
                iso9660) echo "iso9660_x64.efi" ;;
                *) warn "No filesystem driver available for ${BOOT_FSTYPE}" ;;
            esac
            ;;
        aarch64)
            case "$BOOT_FSTYPE" in
                ext3|ext4) echo "aarch64/ext4_aa64.efi" ;;
                btrfs) echo "aarch64/btrfs_aa64.efi" ;;
                *) warn "No aarch64 filesystem driver available for ${BOOT_FSTYPE}" ;;
            esac
            ;;
    esac
}
```

### 7.3 Phase 2 Orchestrator

```bash
phase2_assess_drivers() {
    msg "Phase 2: Assessing filesystem drivers..."

    # Determine /boot filesystem
    if mountpoint -q /boot 2>/dev/null; then
        BOOT_FSTYPE=$(findmnt -n -o FSTYPE /boot 2>/dev/null)
        local boot_src esp_src
        boot_src=$(findmnt -n -o SOURCE /boot 2>/dev/null)
        esp_src=$(findmnt -n -o SOURCE "$ESP" 2>/dev/null)
        if [ "$boot_src" = "$esp_src" ]; then
            NEED_FS_DRIVER=0
        elif [ "$BOOT_FSTYPE" != "vfat" ]; then
            NEED_FS_DRIVER=1
        fi
    else
        # /boot is on root filesystem
        BOOT_FSTYPE=$(findmnt -n -o FSTYPE / 2>/dev/null)
        if [ "$BOOT_FSTYPE" != "vfat" ]; then
            NEED_FS_DRIVER=1
        fi
    fi

    if (( OPT_WITH_DRIVERS == 1 )); then
        NEED_FS_DRIVER=1
    fi

    if (( NEED_FS_DRIVER )); then
        detail "/boot filesystem: ${BOOT_FSTYPE} (driver required)"
        local drivers
        drivers=$(driver_files_for_arch)
        if [ -z "$drivers" ]; then
            warn "Filesystem driver needed for ${BOOT_FSTYPE} but none available."
            warn "LamBoot may not be able to read kernels on /boot."
        else
            ok "Filesystem driver: ${BOOT_FSTYPE} (will install)"
        fi
    else
        detail "/boot filesystem: ${BOOT_FSTYPE:-vfat} (no driver needed)"
    fi
}
```

---

## 8. Phase 3: Boot Entry Discovery

Entry point: `phase3_discover_entries()`

Discovers existing BLS entries and installed kernels. Does NOT generate new
entries (that is Phase 5). Populates arrays used by later phases.

### 8.1 Data Structures

```bash
# Parallel arrays (indexed 0..N-1)
declare -a KERNEL_VERSIONS=()   # e.g., "6.12.0-200.fc43.x86_64"
declare -a KERNEL_PATHS=()      # e.g., "/boot/vmlinuz-6.12.0-200.fc43.x86_64"
declare -a INITRD_PATHS=()      # e.g., "/boot/initramfs-6.12.0-200.fc43.x86_64.img"

declare -a EXISTING_BLS=()      # Filenames of existing BLS .conf on ESP
declare -a EXISTING_UKI=()      # Filenames of UKIs in \EFI\Linux\

KERNEL_CMDLINE=""               # Template command line (from running kernel)
ENTRY_TOKEN=""                  # BLS entry token (machine-id or IMAGE_ID)
HAS_BLS_NATIVE=0                # 1 if distro already has BLS entries
```

### 8.2 Command Line Template

```bash
# get_kernel_cmdline() — extract current kernel cmdline, stripped of boot-specific params
get_kernel_cmdline() {
    if [ -r /etc/kernel/cmdline ]; then
        KERNEL_CMDLINE=$(cat /etc/kernel/cmdline)
    elif [ -r /proc/cmdline ]; then
        KERNEL_CMDLINE=$(sed 's/\bBOOT_IMAGE=[^ ]* *//;s/\binitrd=[^ ]* *//' /proc/cmdline)
    fi
    # Trim whitespace
    KERNEL_CMDLINE=$(echo "$KERNEL_CMDLINE" | xargs)
}
```

### 8.3 Entry Token

```bash
get_entry_token() {
    if [ -r /etc/kernel/entry-token ]; then
        ENTRY_TOKEN=$(cat /etc/kernel/entry-token)
    elif [ -r /etc/os-release ]; then
        ENTRY_TOKEN=$(. /etc/os-release && echo "${IMAGE_ID:-${ID}}")
    fi
    ENTRY_TOKEN="${ENTRY_TOKEN:-$(cat /etc/machine-id 2>/dev/null)}"
    ENTRY_TOKEN="${ENTRY_TOKEN:-linux}"
}
```

### 8.4 Existing BLS Scan

```bash
scan_existing_bls() {
    local bls_dir="${ESP}/${BLS_DIR}"
    if [ -d "$bls_dir" ]; then
        local f
        for f in "$bls_dir"/*.conf; do
            [ -f "$f" ] || continue
            EXISTING_BLS+=("$(basename "$f")")
        done
    fi

    # Also check /boot/loader/entries (Fedora keeps BLS here)
    if [ -d /boot/loader/entries ]; then
        local f
        for f in /boot/loader/entries/*.conf; do
            [ -f "$f" ] || continue
            EXISTING_BLS+=("$(basename "$f")")
        done
    fi

    # Deduplicate
    if (( ${#EXISTING_BLS[@]} > 0 )); then
        local -A seen=()
        local -a deduped=()
        local entry
        for entry in "${EXISTING_BLS[@]}"; do
            if [ -z "${seen[$entry]+x}" ]; then
                seen[$entry]=1
                deduped+=("$entry")
            fi
        done
        EXISTING_BLS=("${deduped[@]}")
        HAS_BLS_NATIVE=1
    fi
}
```

### 8.5 UKI Scan

```bash
scan_existing_uki() {
    local uki_dir="${ESP}/EFI/Linux"
    if [ -d "$uki_dir" ]; then
        local f
        for f in "$uki_dir"/*.efi; do
            [ -f "$f" ] || continue
            EXISTING_UKI+=("$(basename "$f")")
        done
    fi
}
```

### 8.6 Kernel Discovery

```bash
# discover_kernels() — find installed kernels and matching initrds
# Populates KERNEL_VERSIONS, KERNEL_PATHS, INITRD_PATHS
discover_kernels() {
    local kpath version initrd

    # Scan /boot/vmlinuz-*
    for kpath in /boot/vmlinuz-*; do
        [ -f "$kpath" ] || continue
        version="${kpath#/boot/vmlinuz-}"
        initrd=$(find_initrd "$version")

        KERNEL_VERSIONS+=("$version")
        KERNEL_PATHS+=("$kpath")
        INITRD_PATHS+=("$initrd")
    done

    # Arch Linux: /boot/vmlinuz-linux (no version suffix)
    if [ -f /boot/vmlinuz-linux ] && [ "$DISTRO_ID" = "arch" ]; then
        local uname_ver
        uname_ver=$(file /boot/vmlinuz-linux 2>/dev/null | grep -oP 'version \K[^ ]+' || true)
        if [ -z "$uname_ver" ]; then
            uname_ver=$(uname -r)
        fi
        # Avoid duplicate if vmlinuz-$(uname -r) already found
        local dup=0 v
        for v in "${KERNEL_VERSIONS[@]}"; do
            [ "$v" = "$uname_ver" ] && dup=1 && break
        done
        if ! (( dup )); then
            KERNEL_VERSIONS+=("$uname_ver")
            KERNEL_PATHS+=("/boot/vmlinuz-linux")
            INITRD_PATHS+=("$(find_initrd_arch)")
        fi
    fi
}
```

### 8.7 Initrd Resolution (Distro-Aware)

```bash
# find_initrd(version) — return path to initrd for kernel version, or ""
find_initrd() {
    local ver="$1"

    # Distro-specific patterns in priority order
    local -a patterns=()
    case "$DISTRO_ID" in
        fedora|rhel|centos|rocky|alma)
            patterns=(
                "/boot/initramfs-${ver}.img"
            ) ;;
        debian|ubuntu|linuxmint|pop)
            patterns=(
                "/boot/initrd.img-${ver}"
            ) ;;
        opensuse*|sles)
            patterns=(
                "/boot/initrd-${ver}"
            ) ;;
        void)
            patterns=(
                "/boot/initramfs-${ver}.img"
            ) ;;
        gentoo)
            patterns=(
                "/boot/initramfs-${ver}.img"
                "/boot/initramfs-genkernel-*-${ver}"
            ) ;;
        alpine)
            patterns=(
                "/boot/initramfs-${ver}"
            ) ;;
        *)
            # Generic fallback — try all common patterns
            patterns=(
                "/boot/initramfs-${ver}.img"
                "/boot/initrd.img-${ver}"
                "/boot/initrd-${ver}"
            ) ;;
    esac

    local p
    for p in "${patterns[@]}"; do
        # Glob expansion for patterns with wildcards
        local match
        for match in $p; do
            [ -f "$match" ] && echo "$match" && return 0
        done
    done

    echo ""
}

# find_initrd_arch() — Arch Linux specific initrd resolution
find_initrd_arch() {
    if [ -f /boot/initramfs-linux.img ]; then
        echo "/boot/initramfs-linux.img"
    elif [ -f /boot/initramfs-linux-fallback.img ]; then
        echo "/boot/initramfs-linux-fallback.img"
    else
        echo ""
    fi
}
```

### 8.8 Phase 3 Orchestrator

```bash
phase3_discover_entries() {
    msg "Phase 3: Discovering boot entries..."

    get_kernel_cmdline
    get_entry_token
    scan_existing_bls
    scan_existing_uki
    discover_kernels

    detail "Entry token: ${ENTRY_TOKEN}"
    detail "Kernel cmdline: ${KERNEL_CMDLINE}"

    if (( ${#EXISTING_BLS[@]} > 0 )); then
        ok "${#EXISTING_BLS[@]} existing BLS entries found"
    fi
    if (( ${#EXISTING_UKI[@]} > 0 )); then
        ok "${#EXISTING_UKI[@]} UKIs found in \\EFI\\Linux\\"
    fi
    if (( ${#KERNEL_VERSIONS[@]} > 0 )); then
        ok "${#KERNEL_VERSIONS[@]} kernels found in /boot"
    else
        warn "No kernels found in /boot."
    fi

    # NixOS special case
    if [ "$DISTRO_ID" = "nixos" ]; then
        warn "NixOS detected. NixOS uses a generation-based boot model."
        warn "Automatic BLS generation is not supported. Use --no-bls."
        if ! (( OPT_NO_BLS )); then
            OPT_NO_BLS=1
        fi
    fi
}
```

---

## 9. Phase 4: File Installation

Entry point: `phase4_install_files()`

### 9.1 Directory Structure

Created on ESP:
```
EFI/LamBoot/
EFI/LamBoot/drivers/
EFI/LamBoot/modules/
EFI/LamBoot/reports/
loader/entries/        (if BLS entries will be generated)
```

### 9.2 Idempotency via Manifest

```bash
# needs_update(src, dst) — true if dst is missing or differs from src
# Uses sha256 comparison. Returns: 0=needs update, 1=up-to-date
needs_update() {
    local src="$1" dst="$2"
    [ -f "$dst" ] || return 0
    local src_hash dst_hash
    src_hash=$(file_sha256 "$src")
    dst_hash=$(file_sha256 "$dst")
    [ "$src_hash" != "$dst_hash" ]
}
```

### 9.3 Main Binary Installation

```bash
install_efi_binary() {
    local src="${SRC_DIR}/EFI/LamBoot/$(efi_binary)"
    local dst="${ESP}/${EFI_DIR}/$(efi_binary)"

    if needs_update "$src" "$dst"; then
        atomic_copy "$src" "$dst" || die "Failed to install $(efi_binary) to ESP."
        ok "Installed $(efi_binary) ($(( $(stat -c %s "$src") / 1024 )) KiB)"
    else
        detail "$(efi_binary) unchanged, skipping"
    fi
    manifest_add "${EFI_DIR}/$(efi_binary)"
}
```

### 9.4 Fallback Installation

```bash
install_fallback() {
    (( OPT_FALLBACK )) || return 0

    local src="${ESP}/${EFI_DIR}/$(efi_binary)"
    local dst="${ESP}/${FALLBACK_DIR}/$(fallback_name)"
    local backup="${dst}.lamboot-backup"

    run "mkdir -p ${ESP}/${FALLBACK_DIR}" mkdir -p "${ESP}/${FALLBACK_DIR}"

    # Backup existing fallback if not ours and no backup exists yet
    if [ -f "$dst" ] && [ ! -f "$backup" ]; then
        local dst_hash src_hash
        dst_hash=$(file_sha256 "$dst")
        src_hash=$(file_sha256 "$src")
        if [ "$dst_hash" != "$src_hash" ]; then
            run "backup ${dst} -> ${backup}" cp -- "$dst" "$backup" \
                || warn "Failed to backup existing $(fallback_name)"
            ok "Backed up existing $(fallback_name)"
        fi
    fi

    atomic_copy "$src" "$dst" || warn "Failed to install fallback $(fallback_name)"
    ok "Installed fallback $(fallback_name)"
    manifest_add "${FALLBACK_DIR}/$(fallback_name)"
}
```

### 9.5 Driver Installation

```bash
install_drivers() {
    (( NEED_FS_DRIVER )) || return 0

    local drivers
    drivers=$(driver_files_for_arch)
    [ -n "$drivers" ] || return 0

    local drv
    while IFS= read -r drv; do
        local src="${SRC_DIR}/EFI/LamBoot/drivers/${drv}"
        local dst="${ESP}/${EFI_DIR}/drivers/${drv}"
        if [ ! -f "$src" ]; then
            warn "Driver not found: ${src}"
            continue
        fi
        if needs_update "$src" "$dst"; then
            atomic_copy "$src" "$dst" || { warn "Failed to install driver ${drv}"; continue; }
            ok "Installed driver: ${drv}"
        else
            detail "Driver ${drv} unchanged, skipping"
        fi
        manifest_add "${EFI_DIR}/drivers/${drv}"
    done <<< "$drivers"

    # Copy driver license
    local lic_src="${SRC_DIR}/EFI/LamBoot/drivers/LICENSE-GPL-2.0.txt"
    local lic_dst="${ESP}/${EFI_DIR}/drivers/LICENSE-GPL-2.0.txt"
    if [ -f "$lic_src" ]; then
        atomic_copy "$lic_src" "$lic_dst" 2>/dev/null || true
    fi
}
```

### 9.6 Module Installation

```bash
install_modules() {
    (( OPT_WITH_MODULES )) || return 0

    local manifest_src="${SRC_DIR}/EFI/LamBoot/modules/manifest.toml"
    if [ -f "$manifest_src" ]; then
        local dst="${ESP}/${EFI_DIR}/modules/manifest.toml"
        atomic_copy "$manifest_src" "$dst" || warn "Failed to install module manifest"
        manifest_add "${EFI_DIR}/modules/manifest.toml"
    fi

    # Copy module EFI binaries if present
    local mod
    for mod in "${SRC_DIR}"/EFI/LamBoot/modules/*.efi; do
        [ -f "$mod" ] || continue
        local name
        name=$(basename "$mod")
        local dst="${ESP}/${EFI_DIR}/modules/${name}"
        if needs_update "$mod" "$dst"; then
            atomic_copy "$mod" "$dst" || { warn "Failed to install module ${name}"; continue; }
            ok "Installed module: ${name}"
        fi
        manifest_add "${EFI_DIR}/modules/${name}"
    done
}
```

### 9.7 Policy Installation (Config Preservation)

```bash
install_policy() {
    local src="${SRC_DIR}/EFI/LamBoot/policy.toml"
    local dst="${ESP}/${EFI_DIR}/policy.toml"

    [ -f "$src" ] || return 0

    if [ -f "$dst" ]; then
        # Never overwrite existing policy.toml
        local new="${dst}.new"
        if needs_update "$src" "$dst"; then
            atomic_copy "$src" "$new" || true
            detail "New policy template written to policy.toml.new (existing config preserved)"
        fi
    else
        atomic_copy "$src" "$dst" || warn "Failed to install policy.toml"
        ok "Installed policy.toml"
    fi
    # Manifest tracks whichever file exists — existing or newly installed
    manifest_add "${EFI_DIR}/policy.toml"
}
```

### 9.8 Phase 4 Orchestrator

```bash
phase4_install_files() {
    msg "Phase 4: Installing files..."

    # Create directory tree
    local dir
    for dir in "" "/drivers" "/modules" "/reports"; do
        run "mkdir -p ${ESP}/${EFI_DIR}${dir}" mkdir -p "${ESP}/${EFI_DIR}${dir}"
    done
    run "mkdir -p ${ESP}/${BLS_DIR}" mkdir -p "${ESP}/${BLS_DIR}"

    install_efi_binary
    install_fallback
    install_drivers
    install_modules
    install_policy
}
```

---

## 10. Phase 5: BLS Entry Generation

Entry point: `phase5_generate_bls()`

Generates BLS Type 1 entries for distros that do not produce them natively.
Skipped when `OPT_NO_BLS=1` or `HAS_BLS_NATIVE=1`.

### 10.1 Decision Logic

```
IF OPT_NO_BLS:
    skip (user requested)
IF HAS_BLS_NATIVE AND NOT OPT_FORCE:
    skip (distro manages its own entries)
IF KERNEL_VERSIONS is empty:
    skip (nothing to generate for)

FOR each discovered kernel:
    IF "${ENTRY_TOKEN}-${version}.conf" already exists on ESP:
        skip (already present)
    ELSE:
        generate BLS .conf
```

### 10.2 BLS Entry Template

```bash
# generate_bls_entry(index) — write a single BLS .conf file
# index: offset into KERNEL_VERSIONS/KERNEL_PATHS/INITRD_PATHS arrays
# Output file: $ESP/loader/entries/${ENTRY_TOKEN}-${version}.conf
generate_bls_entry() {
    local idx="$1"
    local version="${KERNEL_VERSIONS[$idx]}"
    local kpath="${KERNEL_PATHS[$idx]}"
    local ipath="${INITRD_PATHS[$idx]}"

    local conf_name="${ENTRY_TOKEN}-${version}.conf"
    local conf_path="${ESP}/${BLS_DIR}/${conf_name}"

    # Skip if entry already exists (idempotent)
    if [ -f "$conf_path" ]; then
        detail "BLS entry exists: ${conf_name}"
        manifest_add "${BLS_DIR}/${conf_name}"
        return 0
    fi

    # Compute paths relative to ESP (or /boot if using fs driver)
    local linux_rel initrd_rel
    linux_rel=$(kernel_esp_path "$kpath")
    initrd_rel=$(kernel_esp_path "$ipath")

    local machine_id
    machine_id=$(cat /etc/machine-id 2>/dev/null || echo "unknown")

    local content
    content="title      ${DISTRO_NAME} (${version})
version    ${version}
machine-id ${machine_id}
sort-key   ${ENTRY_TOKEN}
linux      ${linux_rel}
initrd     ${initrd_rel}
options    ${KERNEL_CMDLINE}"

    if (( OPT_DRY_RUN )); then
        msg "  [dry-run] Generate BLS entry: ${conf_name}"
        detail "    title: ${DISTRO_NAME} (${version})"
        return 0
    fi

    local tmp="${conf_path}.lamboot-tmp.$$"
    printf '%s\n' "$content" > "$tmp" || { rm -f "$tmp"; warn "Failed to write BLS entry ${conf_name}"; return 1; }
    mv -f "$tmp" "$conf_path" || { rm -f "$tmp"; warn "Failed to finalize BLS entry ${conf_name}"; return 1; }

    ok "Generated BLS entry: ${conf_name}"
    manifest_add "${BLS_DIR}/${conf_name}"
}
```

### 10.3 Kernel Path Resolution

```bash
# kernel_esp_path(host_path) — convert /boot/vmlinuz-X to ESP-relative path
# If kernel is on ESP (vfat /boot), return path relative to ESP root.
# If kernel is on separate /boot (ext4/btrfs), return /boot-relative path
# (LamBoot reads via filesystem driver).
kernel_esp_path() {
    local hpath="$1"
    [ -n "$hpath" ] || { echo ""; return; }

    # If /boot IS the ESP
    if [ "$ESP" = "/boot" ]; then
        echo "/${hpath#/boot/}"
        return
    fi

    # If kernel lives under ESP mount (e.g., /boot/efi/vmlinuz-X — unusual)
    if [[ "$hpath" == "${ESP}"/* ]]; then
        echo "/${hpath#${ESP}/}"
        return
    fi

    # Kernel is on /boot (separate partition) — path is /boot-relative
    # LamBoot accesses this via filesystem driver
    echo "/${hpath#/boot/}"
}
```

### 10.4 Phase 5 Orchestrator

```bash
phase5_generate_bls() {
    msg "Phase 5: BLS entry generation..."

    if (( OPT_NO_BLS )); then
        detail "BLS generation skipped (--no-bls)"
        return 0
    fi

    if (( HAS_BLS_NATIVE )) && ! (( OPT_FORCE )); then
        ok "Distro provides native BLS entries; skipping generation"
        # Track existing entries in manifest
        local e
        for e in "${EXISTING_BLS[@]}"; do
            if [ -f "${ESP}/${BLS_DIR}/${e}" ]; then
                manifest_add "${BLS_DIR}/${e}"
            fi
        done
        return 0
    fi

    if (( ${#KERNEL_VERSIONS[@]} == 0 )); then
        detail "No kernels discovered; skipping BLS generation"
        return 0
    fi

    local i
    for i in "${!KERNEL_VERSIONS[@]}"; do
        generate_bls_entry "$i" || PARTIAL_FAILURE=1
    done
}
```

---

## 11. Phase 6: UEFI Boot Entry

Entry point: `phase6_efi_boot_entry()`

### 11.1 Prerequisites

```bash
# check_efi_prerequisites() — validate everything needed for efibootmgr
# Calls die on failure, returns 0 on success.
check_efi_prerequisites() {
    # EFI mode
    [ -d /sys/firmware/efi ] \
        || die "Not booted in EFI mode. LamBoot requires UEFI firmware."

    # efibootmgr binary
    command -v efibootmgr >/dev/null 2>&1 \
        || die "efibootmgr not found. Install it:
  Fedora/RHEL: dnf install efibootmgr
  Debian/Ubuntu: apt install efibootmgr
  Arch: pacman -S efibootmgr
  openSUSE: zypper install efibootmgr"

    # efivarfs mounted
    if ! mount | grep -q 'efivarfs'; then
        die "efivarfs not mounted. Mount it:
  mount -t efivarfs efivarfs /sys/firmware/efi/efivars"
    fi

    # efivarfs readable
    ls /sys/firmware/efi/efivars/ >/dev/null 2>&1 \
        || die "Cannot access EFI variables. Check permissions or efivarfs mount."

    # Disk and partition info available
    [ -n "$ESP_DISK" ] && [ -n "$ESP_PARTNUM" ] \
        || die "Cannot determine ESP disk and partition number for efibootmgr.
  ESP device: $(findmnt -n -o SOURCE "$ESP" 2>/dev/null)
  Specify --no-efi-entry to skip boot entry creation."
}
```

### 11.2 Existing Entry Detection

```bash
# find_lamboot_entry() — return boot number of existing LamBoot entry, or ""
find_lamboot_entry() {
    efibootmgr 2>/dev/null \
        | grep -i 'LamBoot' \
        | head -1 \
        | grep -oP 'Boot\K[0-9A-Fa-f]{4}'
}
```

### 11.3 Entry Creation

```bash
create_efi_entry() {
    local existing
    existing=$(find_lamboot_entry)

    if [ -n "$existing" ]; then
        ok "UEFI boot entry already exists: Boot${existing}"
    else
        local loader
        loader=$(efi_loader_path)

        run "create UEFI boot entry" \
            efibootmgr --create \
                --disk "$ESP_DISK" \
                --part "$ESP_PARTNUM" \
                --loader "$loader" \
                --label "$LAMBOOT_LABEL" \
                --quiet \
            || die "efibootmgr failed to create boot entry.
  This may indicate NVRAM is full. Try removing unused entries with:
    efibootmgr -b XXXX -B
  Or install with --fallback --no-efi-entry to use the removable media path."

        existing=$(find_lamboot_entry)
        ok "Created UEFI boot entry: Boot${existing}"
    fi

    # Set as default boot option if requested
    if (( OPT_SET_DEFAULT )); then
        set_default_entry "$existing"
    fi
}
```

### 11.4 Set Default

```bash
# set_default_entry(bootnum) — move LamBoot to first in BootOrder
set_default_entry() {
    local bootnum="$1"
    [ -n "$bootnum" ] || { warn "No boot entry to set as default"; return 1; }

    # Safety: refuse if no valid boot entries would be visible to LamBoot
    local total_entries=0
    (( ${#EXISTING_BLS[@]} > 0 )) && total_entries=${#EXISTING_BLS[@]}
    (( ${#EXISTING_UKI[@]} > 0 )) && total_entries=$(( total_entries + ${#EXISTING_UKI[@]} ))
    (( ${#KERNEL_VERSIONS[@]} > 0 )) && total_entries=$(( total_entries + ${#KERNEL_VERSIONS[@]} ))

    if (( total_entries == 0 )) && ! (( OPT_FORCE )); then
        warn "Refusing --set-default: no boot entries (BLS, UKI, or kernel) detected."
        warn "LamBoot would show an empty menu. Use --force to override."
        PARTIAL_FAILURE=1
        return 1
    fi

    local current_order
    current_order=$(efibootmgr 2>/dev/null | grep '^BootOrder:' | cut -d: -f2 | tr -d ' ')

    # Build new order: LamBoot first, then everything else
    local new_order="${bootnum}"
    local entry
    IFS=',' read -ra entries <<< "$current_order"
    for entry in "${entries[@]}"; do
        [ "$entry" != "$bootnum" ] && new_order="${new_order},${entry}"
    done

    run "set boot order: ${new_order}" \
        efibootmgr --bootorder "$new_order" --quiet \
        || { warn "Failed to set boot order"; PARTIAL_FAILURE=1; return 1; }

    ok "Set LamBoot as default boot option"
}
```

### 11.5 Phase 6 Orchestrator

```bash
phase6_efi_boot_entry() {
    msg "Phase 6: UEFI boot entry..."

    if (( OPT_NO_EFI_ENTRY )); then
        detail "Skipped (--no-efi-entry)"
        return 0
    fi

    if (( IS_CHROOT )); then
        detail "Skipped (chroot environment detected)"
        return 0
    fi

    check_efi_prerequisites
    create_efi_entry
}
```

---

## 12. Phase 7: Systemd Integration

Entry point: `phase7_systemd_integration()`

### 12.1 Mark-Success Service

```bash
install_mark_success_service() {
    local src="${SRC_DIR}/../systemd/lamboot-mark-success.service"
    local dst="${SYSTEMD_UNIT_DIR}/lamboot-mark-success.service"

    [ -f "$src" ] || { detail "lamboot-mark-success.service not found in dist, skipping"; return 0; }

    if needs_update "$src" "$dst"; then
        run "install mark-success service" cp -- "$src" "$dst" \
            || { warn "Failed to install lamboot-mark-success.service"; PARTIAL_FAILURE=1; return 1; }
        ok "Installed lamboot-mark-success.service"
    else
        detail "lamboot-mark-success.service unchanged"
    fi

    if command -v systemctl >/dev/null 2>&1 && ! (( IS_CHROOT )); then
        run "reload systemd" systemctl daemon-reload 2>/dev/null || true
        run "enable mark-success service" systemctl enable lamboot-mark-success.service 2>/dev/null \
            || { warn "Failed to enable lamboot-mark-success.service"; PARTIAL_FAILURE=1; }
    fi
}
```

### 12.2 Kernel-Install Plugin

```bash
install_kernel_install_plugin() {
    local src="${SRC_DIR}/../kernel-install/90-lamboot.install"
    local dst="${KERNEL_INSTALL_DIR}/90-lamboot.install"

    [ -f "$src" ] || { detail "90-lamboot.install not found in dist, skipping"; return 0; }

    if [ -d "$KERNEL_INSTALL_DIR" ]; then
        if needs_update "$src" "$dst"; then
            run "install kernel-install plugin" cp -- "$src" "$dst" \
                || { warn "Failed to install 90-lamboot.install"; PARTIAL_FAILURE=1; return 1; }
            run "chmod +x ${dst}" chmod 755 "$dst" || true
            ok "Installed 90-lamboot.install kernel-install plugin"
        else
            detail "90-lamboot.install unchanged"
        fi
    else
        detail "kernel-install not present on system, skipping plugin"
    fi
}
```

### 12.3 Phase 7 Orchestrator

```bash
phase7_systemd_integration() {
    msg "Phase 7: Systemd integration..."

    install_mark_success_service
    install_kernel_install_plugin
}
```

---

## 13. Phase 8: Post-Install Verification

Entry point: `phase8_verify()`

### 13.1 Verification Checks

```bash
# verify_file(esp_rel_path, description) — check file exists on ESP
# Returns: 0=exists, 1=missing
verify_file() {
    local rel="$1" desc="$2"
    if [ -f "${ESP}/${rel}" ]; then
        local size_kib=$(( $(stat -c %s "${ESP}/${rel}") / 1024 ))
        ok "${desc} (${size_kib} KiB)"
        return 0
    else
        fail "${desc} — MISSING"
        return 1
    fi
}

# verify_bls_entry(conf_path) — validate a single BLS entry
# Checks: kernel file exists, initrd file exists
# Returns: 0=healthy, 1=problems found
verify_bls_entry() {
    local conf="$1"
    local title linux_path initrd_path problems=0

    title=$(grep '^title ' "$conf" 2>/dev/null | sed 's/^title  *//')
    linux_path=$(grep '^linux ' "$conf" 2>/dev/null | sed 's/^linux  *//')
    initrd_path=$(grep '^initrd ' "$conf" 2>/dev/null | sed 's/^initrd  *//')

    local prefix=""
    # Resolve paths: could be ESP-relative or /boot-relative
    for base in "$ESP" /boot; do
        if [ -f "${base}${linux_path}" ]; then
            prefix="$base"
            break
        fi
    done

    local k_status="\xe2\x9c\x93 kernel"
    local i_status="\xe2\x9c\x93 initrd"

    if [ -n "$linux_path" ] && [ ! -f "${prefix}${linux_path}" ]; then
        k_status="\xe2\x9c\x97 kernel MISSING"
        problems=1
    fi

    if [ -n "$initrd_path" ] && [ ! -f "${prefix}${initrd_path}" ]; then
        i_status="\xe2\x9c\x97 initrd MISSING"
        problems=1
    fi

    printf '    %-55s %b  %b\n' "${title:-$(basename "$conf")}" "$k_status" "$i_status"
    return $problems
}
```

### 13.2 Phase 8 Orchestrator

```bash
phase8_verify() {
    msg ""
    msg "Phase 8: Verification"
    msg "---------------------"

    local errors=0

    # Binary
    verify_file "${EFI_DIR}/$(efi_binary)" "$(efi_binary) on ESP" || errors=$((errors+1))

    # Fallback
    if (( OPT_FALLBACK )); then
        verify_file "${FALLBACK_DIR}/$(fallback_name)" "Fallback $(fallback_name)" || errors=$((errors+1))
    fi

    # Drivers
    if (( NEED_FS_DRIVER )); then
        local drv
        while IFS= read -r drv; do
            [ -n "$drv" ] || continue
            verify_file "${EFI_DIR}/drivers/${drv}" "Driver: ${drv}" || errors=$((errors+1))
        done <<< "$(driver_files_for_arch)"
    fi

    # UEFI boot entry
    if ! (( OPT_NO_EFI_ENTRY )) && ! (( IS_CHROOT )); then
        local bootnum
        bootnum=$(find_lamboot_entry)
        if [ -n "$bootnum" ]; then
            ok "UEFI boot entry Boot${bootnum}: ${LAMBOOT_LABEL}"
        else
            fail "UEFI boot entry — NOT FOUND"
            errors=$((errors+1))
        fi
    fi

    # BLS entries (per-entry validation)
    local bls_count=0 bls_problems=0
    local conf
    for conf in "${ESP}/${BLS_DIR}"/*.conf; do
        [ -f "$conf" ] || continue
        bls_count=$((bls_count+1))
        verify_bls_entry "$conf" || bls_problems=$((bls_problems+1))
    done
    if (( bls_count > 0 )); then
        ok "${bls_count} BLS entries (${bls_problems} with problems)"
    else
        if (( ${#EXISTING_UKI[@]} == 0 )); then
            warn "No BLS entries or UKIs found. LamBoot will show an empty menu."
            errors=$((errors+1))
        fi
    fi

    # Systemd integration
    if [ -f "${SYSTEMD_UNIT_DIR}/lamboot-mark-success.service" ]; then
        if command -v systemctl >/dev/null 2>&1 && \
           systemctl is-enabled lamboot-mark-success.service >/dev/null 2>&1; then
            ok "lamboot-mark-success.service enabled"
        else
            detail "lamboot-mark-success.service installed but not enabled"
        fi
    fi
    if [ -f "${KERNEL_INSTALL_DIR}/90-lamboot.install" ]; then
        ok "90-lamboot.install kernel-install plugin"
    fi

    msg ""
    if (( errors == 0 )); then
        msg "LamBoot installed successfully."
    else
        msg "LamBoot installed with ${errors} issue(s). Review warnings above."
    fi

    # Suggest next steps
    if ! (( OPT_SET_DEFAULT )); then
        local current_default
        current_default=$(efibootmgr 2>/dev/null | grep '^BootOrder:' | cut -d: -f2 | cut -d, -f1 | tr -d ' ')
        local current_label
        current_label=$(efibootmgr 2>/dev/null | grep "^Boot${current_default}" | sed 's/^Boot[0-9A-Fa-f]*[* ] //')
        if [ -n "$current_label" ] && ! echo "$current_label" | grep -qi 'lamboot'; then
            msg "Current default bootloader: ${current_label}"
            msg "To make LamBoot the default: lamboot-install --set-default"
        fi
    fi
    msg "To test: reboot and select LamBoot from the UEFI boot menu."
}
```

---

## 14. Install Manifest

### 14.1 Write Manifest

```bash
# write_manifest() — write .install-manifest to ESP after all files are installed
write_manifest() {
    local dst="${ESP}/${MANIFEST_PATH}"
    local tmp="${dst}.lamboot-tmp.$$"
    local ts
    ts=$(date -u '+%Y-%m-%dT%H:%M:%S')

    {
        echo "# LamBoot Install Manifest"
        echo "# Generated: ${ts}"
        echo "# Version: ${LAMBOOT_VERSION}"
        echo "# Arch: ${ARCH}"
        echo "# Distro: ${DISTRO_ID}"
        local entry
        for entry in "${MANIFEST_ENTRIES[@]}"; do
            echo "$entry"
        done
    } > "$tmp" || { rm -f "$tmp"; warn "Failed to write install manifest"; return 1; }

    mv -f "$tmp" "$dst" || { rm -f "$tmp"; warn "Failed to finalize install manifest"; return 1; }
    detail "Wrote install manifest: ${MANIFEST_PATH}"
}
```

### 14.2 Read Manifest

```bash
# read_manifest() — parse .install-manifest, populate arrays
# Sets: MANIFEST_VERSION, MANIFEST_ARCH
# Populates: MANIFEST_HASHES (associative: path -> sha256)
# Returns: 0=found, 1=not found
declare -A MANIFEST_HASHES=()
MANIFEST_VERSION=""
MANIFEST_ARCH=""

read_manifest() {
    local mf="${ESP}/${MANIFEST_PATH}"
    [ -f "$mf" ] || return 1

    MANIFEST_HASHES=()
    local line
    while IFS= read -r line; do
        case "$line" in
            "# Version: "*)  MANIFEST_VERSION="${line#\# Version: }" ;;
            "# Arch: "*)     MANIFEST_ARCH="${line#\# Arch: }" ;;
            "#"*|"")         continue ;;
            sha256:*)
                local hash path
                hash="${line%%  *}"
                path="${line#*  }"
                hash="${hash#sha256:}"
                MANIFEST_HASHES["$path"]="$hash"
                ;;
        esac
    done < "$mf"
    return 0
}
```

---

## 15. Removal (--remove)

Entry point: `do_remove()`

### 15.1 Process

```
1. Read .install-manifest (die if missing and not --force)
2. Remove UEFI boot entry
3. For each manifest entry:
    a. Compute current sha256
    b. If hash matches manifest: delete file
    c. If hash differs: warn "file modified since install, skipping" (unless --force)
4. If --keep-entries: skip BLS .conf files
5. Restore fallback backup if present
6. Disable mark-success service
7. Remove kernel-install plugin
8. Remove empty LamBoot directories
9. Remove manifest itself
```

### 15.2 Implementation

```bash
do_remove() {
    msg "Removing LamBoot installation..."

    phase1_detect_environment

    if ! read_manifest; then
        if (( OPT_FORCE )); then
            warn "No install manifest found. Removing known files with --force."
        else
            die "No install manifest found at ${ESP}/${MANIFEST_PATH}.
  Cannot safely determine which files to remove.
  Use --force to remove known default paths."
        fi
    fi

    # Remove UEFI boot entry
    if ! (( OPT_NO_EFI_ENTRY )) && ! (( IS_CHROOT )); then
        local bootnum
        bootnum=$(find_lamboot_entry)
        if [ -n "$bootnum" ]; then
            run "remove UEFI boot entry Boot${bootnum}" \
                efibootmgr -b "$bootnum" -B --quiet \
                || warn "Failed to remove UEFI boot entry Boot${bootnum}"
            ok "Removed UEFI boot entry Boot${bootnum}"
        fi
    fi

    # Remove manifest-tracked files
    local path hash
    for path in "${!MANIFEST_HASHES[@]}"; do
        local full="${ESP}/${path}"
        [ -f "$full" ] || continue

        # Skip BLS entries if --keep-entries
        if (( OPT_KEEP_ENTRIES )) && [[ "$path" == "${BLS_DIR}/"* ]]; then
            detail "Keeping BLS entry: ${path}"
            continue
        fi

        local current_hash expected_hash
        current_hash=$(file_sha256 "$full")
        expected_hash="${MANIFEST_HASHES[$path]}"

        if [ "$current_hash" = "$expected_hash" ] || (( OPT_FORCE )); then
            run "remove ${path}" rm -f "$full" || warn "Failed to remove ${path}"
            detail "Removed: ${path}"
        else
            warn "File modified since install, skipping: ${path}"
        fi
    done

    # Restore fallback backup
    local fb_backup="${ESP}/${FALLBACK_DIR}/$(fallback_name).lamboot-backup"
    if [ -f "$fb_backup" ]; then
        local fb_dst="${ESP}/${FALLBACK_DIR}/$(fallback_name)"
        run "restore fallback backup" mv -f "$fb_backup" "$fb_dst" \
            || warn "Failed to restore fallback backup"
        ok "Restored original $(fallback_name)"
    fi

    # Disable systemd service
    if command -v systemctl >/dev/null 2>&1 && ! (( IS_CHROOT )); then
        run "disable mark-success service" \
            systemctl disable lamboot-mark-success.service 2>/dev/null || true
    fi
    run "remove mark-success service" \
        rm -f "${SYSTEMD_UNIT_DIR}/lamboot-mark-success.service" 2>/dev/null || true

    # Remove kernel-install plugin
    run "remove kernel-install plugin" \
        rm -f "${KERNEL_INSTALL_DIR}/90-lamboot.install" 2>/dev/null || true

    # Remove empty directories (bottom-up)
    local dir
    for dir in reports modules drivers ""; do
        rmdir "${ESP}/${EFI_DIR}/${dir}" 2>/dev/null || true
    done

    # Remove manifest
    rm -f "${ESP}/${MANIFEST_PATH}" 2>/dev/null

    ok "LamBoot removed."
}
```

---

## 16. Main Entry Point

### 16.1 Option Parsing

```bash
parse_options() {
    while [ $# -gt 0 ]; do
        case "$1" in
            --esp)        [ -n "${2:-}" ] || die "--esp requires a path argument"; OPT_ESP="$2"; shift ;;
            --no-efi-entry) OPT_NO_EFI_ENTRY=1 ;;
            --set-default)  OPT_SET_DEFAULT=1 ;;
            --fallback)     OPT_FALLBACK=1 ;;
            --with-drivers) OPT_WITH_DRIVERS=1 ;;
            --with-modules) OPT_WITH_MODULES=1 ;;
            --remove)       OPT_REMOVE=1 ;;
            --update)       OPT_UPDATE=1 ;;
            --dry-run)      OPT_DRY_RUN=1 ;;
            --force)        OPT_FORCE=1 ;;
            --no-bls)       OPT_NO_BLS=1 ;;
            --keep-entries) OPT_KEEP_ENTRIES=1 ;;
            --quiet)        OPT_QUIET=1 ;;
            --verbose)      OPT_VERBOSE=1 ;;
            --version)      echo "lamboot-install ${LAMBOOT_VERSION}"; exit 0 ;;
            --help|-h)      usage; exit 0 ;;
            *)              die "Unknown option: $1" ;;
        esac
        shift
    done

    # Mutual exclusion
    if (( OPT_REMOVE )); then
        (( OPT_SET_DEFAULT )) && die "--remove and --set-default are mutually exclusive."
        (( OPT_FALLBACK ))    && die "--remove and --fallback are mutually exclusive."
        (( OPT_UPDATE ))      && die "--remove and --update are mutually exclusive."
    fi
    (( OPT_QUIET && OPT_VERBOSE )) && die "--quiet and --verbose are mutually exclusive."
}
```

### 16.2 Usage Text

```bash
usage() {
    cat <<'USAGE'
Usage: lamboot-install [OPTIONS]

Install, update, or remove the LamBoot UEFI bootloader.

Options:
  --esp PATH        Override ESP mount point detection
  --no-efi-entry    Don't create UEFI boot entry (file copy only)
  --set-default     Set LamBoot as the first boot option
  --fallback        Also install as \EFI\BOOT\BOOTX64.EFI
  --with-drivers    Install filesystem drivers (auto-detected by default)
  --with-modules    Install diagnostic modules
  --remove          Remove LamBoot installation (reads install manifest)
  --update          Update existing installation (preserve config)
  --dry-run         Show what would happen without doing it
  --force           Skip safety checks
  --no-bls          Don't generate BLS entries
  --keep-entries    With --remove: keep generated BLS entries
  --quiet           Minimal output
  --verbose         Detailed output
  --version         Print version and exit
  --help            Print this help

Exit codes:
  0  Success
  1  Fatal error
  2  Partial success (non-critical step failed)
  3  Nothing to do (already up to date)
USAGE
}
```

### 16.3 Main

```bash
main() {
    parse_options "$@"

    if (( OPT_REMOVE )); then
        do_remove
        exit $(( PARTIAL_FAILURE ? EXIT_PARTIAL : EXIT_OK ))
    fi

    phase1_detect_environment
    phase2_assess_drivers
    phase3_discover_entries
    phase4_install_files
    phase5_generate_bls
    phase6_efi_boot_entry
    phase7_systemd_integration

    write_manifest
    phase8_verify

    exit $(( PARTIAL_FAILURE ? EXIT_PARTIAL : EXIT_OK ))
}

main "$@"
```

---

## 17. Error Catalog

Every user-facing error message is defined here. Error IDs are for log correlation
and do not appear in output.

| ID   | Context                  | Message                                                                         |
|------|--------------------------|---------------------------------------------------------------------------------|
| E001 | Root check               | `This script must be run as root.`                                              |
| E002 | Architecture             | `Unsupported architecture: <arch>`                                              |
| E003 | Source dir               | `Cannot find LamBoot distribution files.\n  Run from the repository root or install to /usr/share/lamboot.` |
| E004 | EFI binary               | `EFI binary not found: <path>\n  Run ./build.sh first.`                         |
| E005 | ESP override invalid     | `Specified ESP path '<path>' is not a valid EFI System Partition.`              |
| E006 | ESP not found            | `EFI System Partition not found. Tried: standard mounts, findmnt, fstab, block scan.\n  Mount your ESP and retry, or specify it with --esp PATH.` |
| E007 | Not EFI mode             | `Not booted in EFI mode. LamBoot requires UEFI firmware.`                       |
| E008 | efibootmgr missing       | `efibootmgr not found. Install it:\n  Fedora/RHEL: dnf install efibootmgr\n  Debian/Ubuntu: apt install efibootmgr\n  Arch: pacman -S efibootmgr\n  openSUSE: zypper install efibootmgr` |
| E009 | efivarfs not mounted     | `efivarfs not mounted. Mount it:\n  mount -t efivarfs efivarfs /sys/firmware/efi/efivars` |
| E010 | EFI vars inaccessible    | `Cannot access EFI variables. Check permissions or efivarfs mount.`             |
| E011 | ESP disk unknown         | `Cannot determine ESP disk and partition number for efibootmgr.\n  ESP device: <dev>\n  Specify --no-efi-entry to skip boot entry creation.` |
| E012 | efibootmgr create fail   | `efibootmgr failed to create boot entry.\n  This may indicate NVRAM is full. Try removing unused entries with:\n    efibootmgr -b XXXX -B\n  Or install with --fallback --no-efi-entry to use the removable media path.` |
| E013 | EFI binary install fail  | `Failed to install <binary> to ESP.`                                            |
| E014 | Manifest missing (remove)| `No install manifest found at <path>.\n  Cannot safely determine which files to remove.\n  Use --force to remove known default paths.` |
| E015 | Mutual exclusion         | `--<a> and --<b> are mutually exclusive.`                                       |
| E016 | Unknown option           | `Unknown option: <opt>`                                                         |
| E017 | --esp missing arg        | `--esp requires a path argument`                                                |

---

## 18. Failure Modes and Handling

| Failure                     | Detection                              | Behavior                              |
|-----------------------------|----------------------------------------|---------------------------------------|
| ESP not found               | All 5 methods fail                     | die E006                              |
| ESP read-only               | Write test fails                       | die in validate_esp                   |
| Not EFI mode                | `/sys/firmware/efi` missing            | die E007                              |
| efibootmgr missing          | `command -v` check                     | die E008 with per-distro hint         |
| efivarfs not mounted         | `mount` check                          | die E009 with mount command           |
| NVRAM full                   | efibootmgr exit code non-zero          | die E012 with cleanup suggestion      |
| No kernels found             | KERNEL_VERSIONS empty                  | warn, skip BLS, continue              |
| Wrong architecture binary    | efi_binary not in SRC_DIR              | die E004                              |
| Secure Boot + unsigned       | SECURE_BOOT=1                          | warn (advisory, do not block)         |
| Chroot environment           | IS_CHROOT=1                            | Skip efibootmgr + systemd enable      |
| Insufficient ESP space       | df < MIN_ESP_SPACE_KIB                 | validate_esp returns 1                |
| Existing install conflict    | HAS_EXISTING=1 without --update        | Proceed (idempotent — sha256 guards)  |
| NixOS detected               | DISTRO_ID=nixos                        | Force OPT_NO_BLS, warn                |
| File modified since install  | sha256 mismatch on --remove            | Skip file, warn (unless --force)      |

---

## 19. Verification Criteria

### 19.1 Idempotency

Running `lamboot-install` twice in succession must:
- Produce identical ESP contents (byte-for-byte)
- Not create duplicate UEFI boot entries
- Not overwrite policy.toml
- Exit 0 both times (not EXIT_NOOP, because manifest rewrite counts as action)

### 19.2 Dry-Run Fidelity

With `--dry-run`:
- No files created, modified, or deleted on ESP
- No efibootmgr calls executed
- No systemd services installed or enabled
- Output shows all actions that WOULD be taken

### 19.3 Clean Removal

After `lamboot-install` followed by `lamboot-install --remove`:
- No LamBoot files remain on ESP (except policy.toml if modified by user)
- No UEFI boot entry for LamBoot
- Original BOOTX64.EFI restored from backup if --fallback was used
- lamboot-mark-success.service disabled and removed
- 90-lamboot.install removed
- Empty EFI/LamBoot directory tree removed

### 19.4 Non-Destructiveness

At no point does the script:
- Delete files it did not install (enforced by manifest)
- Modify other bootloaders' EFI directories
- Change BootOrder entries other than LamBoot's position
- Overwrite policy.toml without explicit user request

### 19.5 Distro Coverage Matrix

The following must produce a working installation (BLS entries present, binary on ESP,
boot entry created):

| Distro        | BLS Generation | Driver Auto-Detect | Expected |
|---------------|----------------|--------------------|----------|
| Fedora 43     | Skipped (native) | No (vfat ESP = /boot) | Pass |
| Debian 13     | Generated      | Yes (ext4 /boot)   | Pass     |
| Ubuntu 24.04  | Generated      | Yes (ext4 /boot)   | Pass     |
| Arch Linux    | Generated      | Depends on setup   | Pass     |
| openSUSE TW   | Skipped (native) | Depends on setup | Pass     |
| Void Linux    | Generated      | Yes (ext4 /boot)   | Pass     |
| Alpine 3.21   | Generated      | Yes (ext4 /boot)   | Pass     |
| Gentoo        | Generated      | Depends on setup   | Pass     |
| NixOS         | Skipped (warn) | N/A                | Partial  |

---

## 20. Function Index

| Function                      | Section | Purpose                                          |
|-------------------------------|---------|--------------------------------------------------|
| `msg(text)`                   | 5.1     | Informational output                             |
| `detail(text)`                | 5.1     | Verbose output                                   |
| `warn(text)`                  | 5.1     | Warning to stderr                                |
| `die(text)`                   | 5.1     | Fatal error, exit 1                              |
| `ok(text)`                    | 5.1     | Success line with checkmark                      |
| `fail(text)`                  | 5.1     | Failure line with cross                          |
| `run(desc, cmd...)`           | 5.2     | Dry-run wrapper                                  |
| `atomic_copy(src, dst)`       | 5.3     | Crash-safe file copy via temp+mv                 |
| `file_sha256(path)`           | 5.4     | SHA-256 hex digest                               |
| `manifest_add(rel_path)`      | 5.5     | Record file in manifest accumulator              |
| `detect_arch()`               | 6.2     | Set ARCH from uname -m                           |
| `efi_binary()`                | 6.3     | Return EFI filename for current arch             |
| `efi_loader_path()`           | 6.3     | Return backslash EFI path for efibootmgr         |
| `fallback_name()`             | 6.3     | Return fallback filename for current arch        |
| `find_source_dir()`           | 6.4     | Locate dist/ tree                                |
| `detect_chroot()`             | 6.5     | Set IS_CHROOT                                    |
| `detect_secure_boot()`        | 6.6     | Set SECURE_BOOT from EFI vars                    |
| `detect_distro()`             | 6.7     | Set DISTRO_ID, DISTRO_NAME, DISTRO_VERSION       |
| `is_vfat(mp)`                 | 6.8     | Check mount is vfat                              |
| `is_esp_partition(mp)`        | 6.8     | Check partition type GUID                        |
| `validate_esp(mp)`            | 6.8     | Full ESP validation                              |
| `find_esp()`                  | 6.8     | 5-method ESP detection cascade                   |
| `_resolve_esp_device()`       | 6.8     | Populate ESP_DISK and ESP_PARTNUM                |
| `detect_existing()`           | 6.9     | Set HAS_EXISTING                                 |
| `phase1_detect_environment()` | 6.10    | Phase 1 orchestrator                             |
| `driver_files_for_arch()`     | 7.2     | List needed driver filenames                     |
| `phase2_assess_drivers()`     | 7.3     | Phase 2 orchestrator                             |
| `get_kernel_cmdline()`        | 8.2     | Extract template cmdline                         |
| `get_entry_token()`           | 8.3     | Determine BLS entry token                        |
| `scan_existing_bls()`         | 8.4     | Scan for existing BLS .conf files                |
| `scan_existing_uki()`         | 8.5     | Scan for existing UKIs                           |
| `discover_kernels()`          | 8.6     | Find kernels and initrds                         |
| `find_initrd(version)`        | 8.7     | Distro-aware initrd resolution                   |
| `find_initrd_arch()`          | 8.7     | Arch Linux initrd resolution                     |
| `phase3_discover_entries()`   | 8.8     | Phase 3 orchestrator                             |
| `needs_update(src, dst)`      | 9.2     | SHA-256 change detection                         |
| `install_efi_binary()`        | 9.3     | Install main EFI binary                          |
| `install_fallback()`          | 9.4     | Install fallback BOOTX64.EFI                     |
| `install_drivers()`           | 9.5     | Install filesystem drivers                       |
| `install_modules()`           | 9.6     | Install diagnostic modules                       |
| `install_policy()`            | 9.7     | Install policy.toml (config preservation)        |
| `phase4_install_files()`      | 9.8     | Phase 4 orchestrator                             |
| `generate_bls_entry(index)`   | 10.2    | Write single BLS .conf                           |
| `kernel_esp_path(host_path)`  | 10.3    | Convert host path to ESP-relative                |
| `phase5_generate_bls()`       | 10.4    | Phase 5 orchestrator                             |
| `check_efi_prerequisites()`   | 11.1    | Validate EFI environment for efibootmgr          |
| `find_lamboot_entry()`        | 11.2    | Find existing LamBoot UEFI boot entry            |
| `create_efi_entry()`          | 11.3    | Create UEFI boot entry via efibootmgr            |
| `set_default_entry(bootnum)`  | 11.4    | Move LamBoot to first in BootOrder               |
| `phase6_efi_boot_entry()`     | 11.5    | Phase 6 orchestrator                             |
| `install_mark_success_service()` | 12.1 | Install and enable systemd service               |
| `install_kernel_install_plugin()` | 12.2 | Install kernel-install plugin                    |
| `phase7_systemd_integration()`| 12.3    | Phase 7 orchestrator                             |
| `verify_file(rel, desc)`      | 13.1    | Check file exists on ESP                         |
| `verify_bls_entry(conf)`      | 13.1    | Validate BLS entry kernel/initrd references      |
| `phase8_verify()`             | 13.2    | Phase 8 orchestrator                             |
| `write_manifest()`            | 14.1    | Write install manifest to ESP                    |
| `read_manifest()`             | 14.2    | Parse install manifest                           |
| `do_remove()`                 | 15.2    | Full removal procedure                           |
| `parse_options(args...)`      | 16.1    | CLI option parser                                |
| `usage()`                     | 16.2    | Print help text                                  |
| `main(args...)`               | 16.3    | Entry point                                      |
