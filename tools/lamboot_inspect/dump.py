"""Bug-report bundle generator.

Collects every artefact needed to triage a LamBoot boot issue:

* Trust log (``/loader/boot-trust.log``)
* Boot log (``/EFI/LamBoot/reports/boot.log``)
* Boot report (``/EFI/LamBoot/reports/boot.json``)
* Audit log (``/EFI/LamBoot/reports/audit.log``)
* Policy file (``/EFI/LamBoot/policy.toml``)
* EFI variables in the LamBoot GUID namespace (if ``efivar``/``efivarfs``
  are available)
* System context: ``uname -a``, ``lsblk -o+FSTYPE,UUID,PARTUUID``,
  ``blkid``, ``efibootmgr -v``, ``mokutil --sb-state``,
  ``mokutil --list-enrolled``, kernel command line, ``/etc/os-release``
* SHA-256 of every captured artefact (``manifest.sha256``)

The output is a timestamped ``.tar.gz`` that a user can attach to an
issue or email directly.

Design choices:
* All collection commands are best-effort. If ``efibootmgr`` is not
  installed the bundle still produces something useful.
* No sudo is attempted automatically. If a file is unreadable, the
  bundle records the failure in ``collection.log`` and continues.
* The tool never modifies anything — strictly read-only.
"""
from __future__ import annotations

import dataclasses
import datetime as dt
import hashlib
import os
import pathlib
import shutil
import subprocess
import tarfile
import tempfile
from typing import Optional


@dataclasses.dataclass
class Collected:
    source: str
    archive_path: str
    size_bytes: int
    sha256: str


@dataclasses.dataclass
class CollectionLog:
    collected: list
    skipped: list
    started: dt.datetime
    finished: Optional[dt.datetime]
    bundle_path: Optional[pathlib.Path]

    def as_text(self) -> str:
        lines = [f"# lamboot-inspect dump — {self.started.isoformat()}"]
        if self.finished:
            span = (self.finished - self.started).total_seconds()
            lines.append(f"# duration: {span:.3f}s")
        lines.append("")
        lines.append("# Collected")
        for c in self.collected:
            lines.append(
                f"{c.sha256}  {c.size_bytes:>10}  {c.archive_path}  (from {c.source})"
            )
        if self.skipped:
            lines.append("")
            lines.append("# Skipped / errors")
            for entry in self.skipped:
                lines.append(entry)
        return "\n".join(lines) + "\n"


# Candidate locations for each ESP-side artefact. Tried in order.
DEFAULT_ESP_MOUNTS = (
    pathlib.Path("/boot/efi"),
    pathlib.Path("/efi"),
    pathlib.Path("/boot"),
)

ESP_ARTEFACTS = (
    # (on-ESP relative path, bundle archive path)
    ("loader/boot-trust.log", "trust/boot-trust.log"),
    ("EFI/LamBoot/reports/boot.log", "reports/boot.log"),
    ("EFI/LamBoot/reports/boot.json", "reports/boot.json"),
    ("EFI/LamBoot/reports/audit.log", "reports/audit.log"),
    ("EFI/LamBoot/reports/error.json", "reports/error.json"),
    ("EFI/LamBoot/policy.toml", "config/policy.toml"),
    ("EFI/LamBoot/modules/manifest.toml", "config/manifest.toml"),
)


# Optional system-probe commands. Each tuple is
# (archive name, argv, allowed-to-fail).
PROBES = (
    ("system/uname.txt", ["uname", "-a"], False),
    ("system/os-release.txt", ["cat", "/etc/os-release"], True),
    ("system/cmdline.txt", ["cat", "/proc/cmdline"], True),
    ("system/lsblk.txt", ["lsblk", "-o", "NAME,SIZE,FSTYPE,UUID,PARTUUID,MOUNTPOINT"], True),
    ("system/blkid.txt", ["blkid"], True),
    ("system/efibootmgr.txt", ["efibootmgr", "-v"], True),
    ("system/mokutil-sbstate.txt", ["mokutil", "--sb-state"], True),
    ("system/mokutil-enrolled.txt", ["mokutil", "--list-enrolled"], True),
)


# EFI variable names in the LamBoot GUID namespace (4C414D42-...).
# Exported via efivarfs if present.
LAMBOOT_VARS = (
    "LamBootCrashCounter",
    "LamBootLastBootStatus",
    "LamBootFallbackEnable",
    "LamBootTelemetry",
)
LAMBOOT_GUID_SUFFIX = "4c414d42-4f4f-5400-0000-000000000001"


def find_esp_mount() -> Optional[pathlib.Path]:
    """Locate the ESP mount point, trying standard locations first."""
    for candidate in DEFAULT_ESP_MOUNTS:
        if candidate.is_dir() and (candidate / "EFI").exists():
            return candidate
    # Ask findmnt as a fallback.
    try:
        out = subprocess.run(
            ["findmnt", "-no", "TARGET", "-t", "vfat", "--source", "/dev/disk/by-partuuid"],
            capture_output=True,
            text=True,
            check=False,
            timeout=5,
        )
        for line in out.stdout.splitlines():
            p = pathlib.Path(line.strip())
            if p.is_dir() and (p / "EFI").exists():
                return p
    except FileNotFoundError:
        pass
    return None


def _sha256_path(path: pathlib.Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(65536), b""):
            h.update(chunk)
    return h.hexdigest()


def _copy_into(
    source: pathlib.Path,
    staging_root: pathlib.Path,
    archive_rel: str,
    collected: list,
    skipped: list,
) -> None:
    destination = staging_root / archive_rel
    destination.parent.mkdir(parents=True, exist_ok=True)
    try:
        shutil.copy2(source, destination)
    except OSError as e:
        skipped.append(f"{source}: {e}")
        return
    try:
        digest = _sha256_path(destination)
        size = destination.stat().st_size
    except OSError as e:
        skipped.append(f"{destination}: post-copy stat: {e}")
        return
    collected.append(
        Collected(
            source=str(source),
            archive_path=archive_rel,
            size_bytes=size,
            sha256=digest,
        )
    )


def _run_probe(
    argv: list,
    staging_root: pathlib.Path,
    archive_rel: str,
    collected: list,
    skipped: list,
    allow_fail: bool,
) -> None:
    tool = argv[0]
    if shutil.which(tool) is None:
        if allow_fail:
            skipped.append(f"probe {' '.join(argv)}: tool not installed")
            return
        skipped.append(f"probe {' '.join(argv)}: tool not installed (required)")
        return
    dest = staging_root / archive_rel
    dest.parent.mkdir(parents=True, exist_ok=True)
    try:
        result = subprocess.run(
            argv,
            capture_output=True,
            text=True,
            check=False,
            timeout=10,
        )
    except (OSError, subprocess.SubprocessError) as e:
        skipped.append(f"probe {' '.join(argv)}: {e}")
        return
    body = (
        f"$ {' '.join(argv)}\n"
        f"# exit {result.returncode}\n\n"
        f"{result.stdout}\n"
    )
    if result.stderr:
        body += "# stderr\n" + result.stderr
    dest.write_text(body, encoding="utf-8")
    try:
        digest = _sha256_path(dest)
        size = dest.stat().st_size
    except OSError as e:
        skipped.append(f"probe {' '.join(argv)}: stat after write: {e}")
        return
    collected.append(
        Collected(
            source="probe:" + " ".join(argv),
            archive_path=archive_rel,
            size_bytes=size,
            sha256=digest,
        )
    )


def _collect_efi_vars(staging_root: pathlib.Path, collected: list, skipped: list) -> None:
    efivarfs = pathlib.Path("/sys/firmware/efi/efivars")
    if not efivarfs.is_dir():
        skipped.append("efivars: /sys/firmware/efi/efivars not mounted")
        return
    dest_dir = staging_root / "efivars"
    dest_dir.mkdir(parents=True, exist_ok=True)
    any_found = False
    for var_name in LAMBOOT_VARS:
        candidate = efivarfs / f"{var_name}-{LAMBOOT_GUID_SUFFIX}"
        if not candidate.exists():
            continue
        any_found = True
        dest = dest_dir / candidate.name
        try:
            shutil.copy2(candidate, dest)
        except OSError as e:
            skipped.append(f"efivar {var_name}: {e}")
            continue
        try:
            digest = _sha256_path(dest)
            size = dest.stat().st_size
        except OSError as e:
            skipped.append(f"efivar {var_name}: stat: {e}")
            continue
        collected.append(
            Collected(
                source=str(candidate),
                archive_path=f"efivars/{candidate.name}",
                size_bytes=size,
                sha256=digest,
            )
        )
    if not any_found:
        skipped.append("efivars: no LamBoot variables present")


def run(
    output: pathlib.Path,
    esp_mount: Optional[pathlib.Path] = None,
) -> CollectionLog:
    """Produce a diagnostic bundle and return the collection log.

    Args:
        output: Path where the .tar.gz bundle will be written.
        esp_mount: Override ESP mount detection (useful for testing).

    Returns:
        The :class:`CollectionLog`. The ``bundle_path`` field is set
        to ``output`` on success.
    """
    started = dt.datetime.now()
    collected: list = []
    skipped: list = []

    if esp_mount is None:
        esp_mount = find_esp_mount()

    with tempfile.TemporaryDirectory(prefix="lamboot-inspect-") as tmp:
        staging = pathlib.Path(tmp) / "lamboot-inspect-dump"
        staging.mkdir()

        if esp_mount is not None:
            for rel, archive_rel in ESP_ARTEFACTS:
                source = esp_mount / rel
                if source.exists():
                    _copy_into(source, staging, archive_rel, collected, skipped)
                else:
                    skipped.append(f"ESP artefact missing: {source}")
        else:
            skipped.append("ESP: could not locate mount point")

        for archive_rel, argv, allow_fail in PROBES:
            _run_probe(argv, staging, archive_rel, collected, skipped, allow_fail)

        _collect_efi_vars(staging, collected, skipped)

        log = CollectionLog(
            collected=collected,
            skipped=skipped,
            started=started,
            finished=dt.datetime.now(),
            bundle_path=output,
        )
        (staging / "collection.log").write_text(log.as_text(), encoding="utf-8")

        # Manifest: repeat the sha256 + path list for quick scanning.
        manifest = "\n".join(
            f"{c.sha256}  {c.archive_path}" for c in collected
        )
        (staging / "manifest.sha256").write_text(manifest + "\n", encoding="utf-8")

        output.parent.mkdir(parents=True, exist_ok=True)
        with tarfile.open(output, "w:gz") as tar:
            tar.add(staging, arcname="lamboot-inspect-dump")

    log.finished = dt.datetime.now()
    return log
