"""Output renderers.

Three modes:

- ``text``     — coloured tabular output for humans on a terminal.
- ``json``     — structured machine-readable output.
- ``timeline`` — sparse time-ordered ASCII with phase duration bars.

Color use: ANSI only, auto-disabled when the output stream is not a TTY
or when ``NO_COLOR`` is set in the environment (per the NO_COLOR
convention, no-color.org).
"""
from __future__ import annotations

import dataclasses
import json
import os
import sys
from typing import Iterable, Optional, TextIO

from . import trust_log as tl
from . import boot_log as bl
from . import audit
from . import report as report_mod


# ---------------------------------------------------------------------------
# Colour handling
# ---------------------------------------------------------------------------


class Color:
    RESET = "\x1b[0m"
    BOLD = "\x1b[1m"
    DIM = "\x1b[2m"
    RED = "\x1b[31m"
    GREEN = "\x1b[32m"
    YELLOW = "\x1b[33m"
    BLUE = "\x1b[34m"
    MAGENTA = "\x1b[35m"
    CYAN = "\x1b[36m"
    WHITE = "\x1b[37m"


def _supports_color(stream: TextIO) -> bool:
    if "NO_COLOR" in os.environ:
        return False
    if os.environ.get("LAMBOOT_DIAG_FORCE_COLOR") == "1":
        return True
    return hasattr(stream, "isatty") and stream.isatty()


class _Styler:
    def __init__(self, enabled: bool) -> None:
        self.enabled = enabled

    def wrap(self, text: str, *codes: str) -> str:
        if not self.enabled or not codes:
            return text
        prefix = "".join(codes)
        return f"{prefix}{text}{Color.RESET}"


# ---------------------------------------------------------------------------
# Trust-log rendering
# ---------------------------------------------------------------------------

_CLASS_SYMBOL = {"ok": "[OK]", "fail": "[FAIL]", "warn": "[WARN]", "info": "[--]"}
_CLASS_COLOR = {
    "ok": (Color.GREEN,),
    "fail": (Color.RED, Color.BOLD),
    "warn": (Color.YELLOW,),
    "info": (Color.DIM,),
}


def render_trust_log_text(
    result: tl.ParseResult,
    stream: TextIO,
    *,
    filter_event: Optional[str] = None,
    show_errors_only: bool = False,
    show_sha: bool = True,
) -> None:
    """Pretty-print a parsed trust log to ``stream`` as text."""
    styler = _Styler(_supports_color(stream))

    if result.source_path:
        stream.write(
            styler.wrap(f"Source: {result.source_path}\n", Color.DIM)
        )
    if result.source_sha256:
        stream.write(
            styler.wrap(f"SHA-256: {result.source_sha256}\n", Color.DIM)
        )
    stream.write(
        styler.wrap(
            f"Events: {len(result.events)} "
            f"(failures: {len(result.failures())}, warnings: {len(result.warnings())})\n",
            Color.BOLD,
        )
    )
    stream.write("\n")

    for event in result.events:
        if filter_event and event.event != filter_event:
            continue
        cls = event.classification()
        if show_errors_only and cls not in ("fail", "warn"):
            continue
        symbol = styler.wrap(_CLASS_SYMBOL[cls], *_CLASS_COLOR[cls])
        seq = f"#{event.seq:<4}" if event.seq else "       "
        line = f"{symbol} {seq} {event.event}"
        stream.write(line)
        if event.path:
            stream.write(f"  path={event.path}")
        if event.verified_via:
            colour = Color.GREEN if event.verified_via in ("shim_mok", "shim_vendor") else Color.YELLOW
            stream.write("  via=" + styler.wrap(event.verified_via, colour))
        if show_sha and event.sha256:
            stream.write(f"  sha256={event.sha256[:16]}…")
        if event.size:
            stream.write(f"  size={event.size}")
        if event.status and event.status not in ("SUCCESS", "Success"):
            stream.write(f"  status={styler.wrap(event.status, Color.YELLOW)}")
        if event.note:
            stream.write(f"\n    note: {event.note}")
        stream.write("\n")

    if result.parse_errors:
        stream.write("\n")
        stream.write(styler.wrap("Parse errors:\n", Color.RED, Color.BOLD))
        for err in result.parse_errors:
            stream.write(f"  line {err.line_number}: {err.reason}\n")
            stream.write(styler.wrap(f"    raw: {err.raw.strip()}\n", Color.DIM))

    if result.schema_violations:
        stream.write("\n")
        stream.write(styler.wrap("Schema violations:\n", Color.YELLOW, Color.BOLD))
        for v in result.schema_violations:
            stream.write(f"  line {v.line_number}: {v.event.event}\n")
            for issue in v.issues:
                stream.write(f"    - {issue}\n")


def render_trust_log_json(result: tl.ParseResult, stream: TextIO) -> None:
    out = {
        "source_path": str(result.source_path) if result.source_path else None,
        "source_sha256": result.source_sha256,
        "events": [dataclasses.asdict(e) for e in result.events],
        "parse_errors": [dataclasses.asdict(e) for e in result.parse_errors],
        "schema_violations": [
            {
                "line_number": v.line_number,
                "event": dataclasses.asdict(v.event),
                "issues": list(v.issues),
            }
            for v in result.schema_violations
        ],
    }
    json.dump(out, stream, indent=2, default=str)
    stream.write("\n")


def render_trust_log_timeline(result: tl.ParseResult, stream: TextIO) -> None:
    """Compact time-ordered sparkline-style rendering.

    Each event is a single short line highlighting only the load-bearing
    fields, designed to scan at 80 columns.
    """
    styler = _Styler(_supports_color(stream))
    for event in result.events:
        cls = event.classification()
        symbol = styler.wrap(_CLASS_SYMBOL[cls], *_CLASS_COLOR[cls])
        pieces = [symbol, event.event]
        if event.path:
            pieces.append(event.path.rsplit("/", 1)[-1])
        if event.verified_via:
            pieces.append(f"via={event.verified_via}")
        if event.size:
            pieces.append(f"{event.size}b")
        stream.write(" ".join(pieces) + "\n")


def render_stats(stats: tl.Statistics, stream: TextIO) -> None:
    styler = _Styler(_supports_color(stream))
    stream.write(styler.wrap("Trust-log statistics\n", Color.BOLD))
    stream.write(f"  Total events:        {stats.total_events}\n")
    stream.write(f"  Boot attempts:       {stats.boot_attempts}\n")
    stream.write(f"  Failures:            {stats.failures}\n")
    stream.write(f"  Warnings/degraded:   {stats.warnings}\n")
    if stats.latest_verified_path:
        stream.write(f"  Last verified path:  {stats.latest_verified_path}\n")
    if stats.latest_verified_sha256:
        stream.write(f"  Last verified SHA:   {stats.latest_verified_sha256}\n")
    if stats.sha256_verify_vs_load_match is not None:
        match = (
            styler.wrap("match", Color.GREEN)
            if stats.sha256_verify_vs_load_match
            else styler.wrap("MISMATCH (SDS-4 §6.4 invariant violated)", Color.RED, Color.BOLD)
        )
        stream.write(f"  Verify↔load SHA:     {match}\n")
    if stats.by_event:
        stream.write("\n  Events by type:\n")
        for name, count in sorted(stats.by_event.items(), key=lambda x: -x[1]):
            stream.write(f"    {count:>5}  {name}\n")
    if stats.by_verified_via:
        stream.write("\n  verified_via distribution:\n")
        for tok, count in sorted(stats.by_verified_via.items(), key=lambda x: -x[1]):
            stream.write(f"    {count:>5}  {tok}\n")


# ---------------------------------------------------------------------------
# Boot-log rendering
# ---------------------------------------------------------------------------


_LEVEL_COLOR = {
    bl.Level.DEBUG: (Color.DIM,),
    bl.Level.INFO: (Color.WHITE,),
    bl.Level.WARN: (Color.YELLOW,),
    bl.Level.ERROR: (Color.RED, Color.BOLD),
    bl.Level.UNKNOWN: (Color.DIM,),
}


def render_boot_log_text(
    parsed: bl.ParsedBootLog,
    stream: TextIO,
    *,
    filter_level: Optional[bl.Level] = None,
    show_errors_only: bool = False,
) -> None:
    styler = _Styler(_supports_color(stream))
    if parsed.header_version or parsed.header_arch:
        stream.write(
            styler.wrap(
                f"LamBoot {parsed.header_version or '?'} "
                f"({parsed.header_arch or '?'}) boot log\n",
                Color.BOLD,
            )
        )
    if parsed.header_timestamp:
        stream.write(f"Started: {parsed.header_timestamp.isoformat()}\n")
    total = parsed.total_duration()
    if total is not None:
        stream.write(f"Span:    {total:.3f}s\n")
    stream.write("\n")

    for entry in parsed.entries:
        if filter_level and entry.level != filter_level:
            continue
        if show_errors_only and entry.level not in (bl.Level.ERROR, bl.Level.WARN):
            continue
        ts = entry.timestamp.isoformat() if entry.timestamp else "---"
        level = styler.wrap(f"{entry.level.value:<5}", *_LEVEL_COLOR[entry.level])
        prefix = "  " if entry.continuation else ""
        stream.write(f"{ts}  {level}  {prefix}{entry.message}\n")

    if parsed.phases:
        stream.write("\n")
        stream.write(styler.wrap("Phase timings:\n", Color.BOLD))
        for phase in parsed.phases:
            if phase.duration_seconds is not None:
                bar = "█" * max(1, int(phase.duration_seconds * 4))
                stream.write(
                    f"  {phase.name:<12} {phase.duration_seconds:.3f}s  {bar}\n"
                )
            else:
                stream.write(
                    f"  {phase.name:<12} "
                    f"{styler.wrap('no timestamps', Color.DIM)}\n"
                )


def render_boot_log_json(parsed: bl.ParsedBootLog, stream: TextIO) -> None:
    out = {
        "header": {
            "version": parsed.header_version,
            "arch": parsed.header_arch,
            "timestamp": parsed.header_timestamp.isoformat()
            if parsed.header_timestamp
            else None,
        },
        "entries": [
            {
                "line_number": e.line_number,
                "timestamp": e.timestamp.isoformat() if e.timestamp else None,
                "level": e.level.value,
                "message": e.message,
                "continuation": e.continuation,
            }
            for e in parsed.entries
        ],
        "phases": [
            {
                "name": p.name,
                "start": p.start.isoformat() if p.start else None,
                "end": p.end.isoformat() if p.end else None,
                "duration_seconds": p.duration_seconds,
                "start_line": p.start_line,
                "end_line": p.end_line,
            }
            for p in parsed.phases
        ],
        "total_duration_seconds": parsed.total_duration(),
    }
    json.dump(out, stream, indent=2, default=str)
    stream.write("\n")


# ---------------------------------------------------------------------------
# Summary: boot.json + audit.log + last trust events
# ---------------------------------------------------------------------------


def render_summary_text(
    trust: Optional[tl.ParseResult],
    boot: Optional[bl.ParsedBootLog],
    report: Optional[report_mod.BootReport],
    audit_parsed: Optional[audit.ParsedAudit],
    stream: TextIO,
) -> None:
    styler = _Styler(_supports_color(stream))
    stream.write(styler.wrap("=== LamBoot last-boot summary ===\n\n", Color.BOLD))
    if report is not None:
        stream.write(f"LamBoot:     {report.lamboot_version} ({report.lamboot_arch})\n")
        stream.write(f"Booted:      {report.timestamp}\n")
        stream.write(f"Entry:       {report.entry_name} [{report.entry_id}]\n")
        stream.write(f"Entry type:  {report.entry_type}\n")
        if report.path:
            stream.write(f"Path:        {report.path}\n")
        if report.os_name:
            stream.write(f"OS:          {report.os_name}\n")
        if report.system_manufacturer or report.system_product:
            stream.write(
                f"System:      {report.system_manufacturer} {report.system_product}\n"
            )
        if report.hypervisor:
            stream.write(f"Hypervisor:  {report.hypervisor}\n")
        if report.vmid:
            stream.write(f"VMID:        {report.vmid}\n")
        if report.fleet_id:
            stream.write(f"Fleet:       {report.fleet_id}\n")
        if report.iommu_present:
            stream.write(f"IOMMU:       {report.iommu} ({report.iommu_units} units)\n")
        if report.boot_timing_ms:
            stream.write(f"Timing:      {report.boot_timing_ms}\n")
    else:
        stream.write(styler.wrap("(no boot.json report available)\n", Color.DIM))

    if boot is not None:
        total = boot.total_duration()
        if total is not None:
            stream.write(f"Boot span:   {total:.3f}s\n")
        errors = boot.errors()
        if errors:
            stream.write(
                styler.wrap(
                    f"Boot-log issues: {len(errors)} (use 'boot-log --errors' for details)\n",
                    Color.YELLOW,
                )
            )

    if trust is not None:
        stream.write("\n")
        stream.write(styler.wrap("Trust chain:\n", Color.BOLD))
        verified = trust.find_verified_load()
        loaded = trust.find_loaded_native()
        attempt = trust.boot_attempt()
        if verified is not None:
            stream.write(
                f"  verified:    {verified.path}  "
                f"via={verified.verified_via}  "
                f"sha={verified.sha256[:16] if verified.sha256 else '—'}…\n"
            )
        else:
            stream.write(styler.wrap("  verified:    (no image_verified event)\n", Color.DIM))
        if loaded is not None:
            stream.write(
                f"  loaded:      {loaded.path}  "
                f"sha={loaded.sha256[:16] if loaded.sha256 else '—'}…\n"
            )
            if (
                verified is not None
                and loaded.sha256
                and verified.sha256
                and loaded.sha256 != verified.sha256
            ):
                stream.write(
                    styler.wrap(
                        "  TOCTOU:      SHA-256 mismatch between verify and load!\n",
                        Color.RED,
                        Color.BOLD,
                    )
                )
        if attempt is not None:
            stream.write(f"  attempt:     {attempt.path or attempt.note}\n")

        failures = trust.failures()
        if failures:
            stream.write("\n")
            stream.write(styler.wrap(f"  Failures ({len(failures)}):\n", Color.RED))
            for f in failures[:5]:
                stream.write(f"    - {f.event}: {f.note or f.verified_via}\n")
            if len(failures) > 5:
                stream.write(f"    ... ({len(failures) - 5} more)\n")
    else:
        stream.write("\n")
        stream.write(styler.wrap("(no boot-trust.log available)\n", Color.DIM))

    if audit_parsed is not None and audit_parsed.boots():
        recent = audit_parsed.boots()[-5:]
        stream.write("\n")
        stream.write(styler.wrap("Recent boot history:\n", Color.BOLD))
        for e in recent:
            ts = e.timestamp.isoformat() if e.timestamp else "---"
            stream.write(f"  {ts}  {e.detail}\n")
