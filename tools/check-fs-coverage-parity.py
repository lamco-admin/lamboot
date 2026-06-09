#!/usr/bin/env python3
"""Enforce that the installer and the bootloader agree on which filesystems
LamBoot reads natively — so the two lists can never silently drift again.

Two independently-maintained lists answer "does LamBoot have a native backend
for this filesystem (so the legacy EfiFs driver must be skipped)?":

  * bash:  tools/lamboot-install :: is_filesystem_natively_covered()
  * rust:  lamboot-core/src/drivers.rs :: filesystem_natively_covered()

Drift between them is exactly the v0.16.x regression class: the runtime gained
native xfs/zfs readers (and skips their legacy drivers — load-bearing, since a
loaded EfiFs driver attaches BY_DRIVER and shadows the native backend), but the
installer kept shipping those drivers and the capabilities JSON kept advertising
them as via-driver. This check fails the build when the two diverge.

Runs in the pre-commit hook and CI, alongside tools/check-layers.py.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
INSTALLER = ROOT / "tools" / "lamboot-install"
DRIVERS_RS = ROOT / "lamboot-core" / "src" / "drivers.rs"
KERNEL_HOOK = ROOT / "tools" / "lamboot-kernel-hook"

# Normalize concrete fstypes / driver prefixes to a filesystem family, so
# ext2/ext3/ext4 -> ext and fat/vfat/fat32 -> fat compare equal across the two
# spellings. Unknown tokens map to themselves, so a newly-added backend on one
# side surfaces as a mismatch rather than being silently absorbed.
FAMILY = {
    "ext2": "ext",
    "ext3": "ext",
    "ext4": "ext",
    "fat": "fat",
    "vfat": "fat",
    "fat32": "fat",
    "btrfs": "btrfs",
    "xfs": "xfs",
    "zfs": "zfs",
}


def fam(token: str) -> str:
    return FAMILY.get(token, token)


def bash_families() -> set[str]:
    text = INSTALLER.read_text()
    m = re.search(
        r"is_filesystem_natively_covered\(\)\s*\{(.*?)\n\}", text, re.S
    )
    if not m:
        sys.exit(
            "FAIL: could not locate is_filesystem_natively_covered() in "
            "tools/lamboot-install"
        )
    arm = re.search(r"^\s*([A-Za-z0-9|]+)\)\s*return 0", m.group(1), re.M)
    if not arm:
        sys.exit(
            "FAIL: no `return 0` case arm in is_filesystem_natively_covered()"
        )
    return {fam(t) for t in arm.group(1).split("|") if t}


def rust_families() -> set[str]:
    text = DRIVERS_RS.read_text()
    m = re.search(
        r"fn filesystem_natively_covered\([^)]*\)\s*->\s*bool\s*\{(.*?)\n\}",
        text,
        re.S,
    )
    if not m:
        sys.exit(
            "FAIL: could not locate filesystem_natively_covered() in "
            "lamboot-core/src/drivers.rs"
        )
    prefixes = re.findall(r'starts_with\("([a-z0-9]+)_"\)', m.group(1))
    if not prefixes:
        sys.exit(
            'FAIL: no starts_with("<fs>_") patterns in '
            "filesystem_natively_covered()"
        )
    return {fam(p) for p in prefixes}


def hook_placement_families() -> set[str]:
    """The filesystems the kernel hook writes BLS entries in-place on /boot for
    (resolve_bls_dir's `BLS_DIR="/boot/loader/entries"` case arm)."""
    text = KERNEL_HOOK.read_text()
    m = re.search(
        r'\n\s*([A-Za-z0-9|]+)\)\s*BLS_DIR="/boot/loader/entries"', text
    )
    if not m:
        sys.exit(
            'FAIL: could not locate the boot_in_place case arm '
            '(BLS_DIR="/boot/loader/entries") in tools/lamboot-kernel-hook'
        )
    return {fam(t) for t in m.group(1).split("|") if t}


def main() -> int:
    bash = bash_families()
    rust = rust_families()
    if bash != rust:
        print("FAIL: installer vs runtime native-filesystem coverage DIVERGED")
        print(f"  bash (tools/lamboot-install)       : {', '.join(sorted(bash)) or '(none)'}")
        print(f"  rust (lamboot-core/src/drivers.rs) : {', '.join(sorted(rust)) or '(none)'}")
        if bash - rust:
            print(
                f"  only in installer : {', '.join(sorted(bash - rust))} "
                "(installer claims native coverage the runtime does not)"
            )
        if rust - bash:
            print(
                f"  only in runtime   : {', '.join(sorted(rust - bash))} "
                "(installer would ship/advertise a legacy driver the runtime skips)"
            )
        print("  -> update whichever side is stale so both lists match.")
        return 1
    print(f"fs-coverage parity OK — both cover: {', '.join(sorted(bash))}")

    # Placement parity: the kernel hook writes BLS entries in-place on /boot for
    # exactly the natively-readable filesystems EXCEPT zfs (lamzfs read-in-place
    # is deferred until field-soaked → ESP placement). If a native backend is
    # added to is_filesystem_natively_covered but resolve_bls_dir is not updated,
    # a kernel upgrade on that /boot fs writes a misplaced, unbootable ESP entry —
    # the v0.16.3 RHEL/xfs regression. Tie the two so they cannot drift.
    hook = hook_placement_families()
    inplace = bash - {"zfs"}
    if hook != inplace:
        print("FAIL: kernel-hook BLS placement DIVERGED from installer boot_in_place set")
        print(f"  hook (lamboot-kernel-hook resolve_bls_dir) : {', '.join(sorted(hook)) or '(none)'}")
        print(f"  expected (native coverage minus zfs)       : {', '.join(sorted(inplace))}")
        if inplace - hook:
            print(
                f"  MISSING in hook : {', '.join(sorted(inplace - hook))} "
                "(kernel upgrades on this /boot fs get a misplaced ESP entry)"
            )
        if hook - inplace:
            print(f"  only in hook    : {', '.join(sorted(hook - inplace))}")
        print("  -> update resolve_bls_dir in tools/lamboot-kernel-hook to match.")
        return 1
    print(f"bls-placement parity OK — hook writes in-place for: {', '.join(sorted(hook))}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
