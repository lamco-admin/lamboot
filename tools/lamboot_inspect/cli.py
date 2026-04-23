"""Command-line interface.

All subcommand dispatch and argument handling lives here. The
individual subcommand handlers are small adapters over the parser /
renderer modules — keeping CLI policy out of the data code and making
every subcommand independently testable.

Exit codes (stable across minor versions):

* 0 — success
* 1 — usage error (unknown subcommand, missing required argument)
* 2 — source file unreadable (path not found, permission denied)
* 3 — parse error (trust log / boot log malformed beyond recovery)
* 4 — schema violation (records parsed but fail the SDS-4 §6 schema)
* 5 — verification failed (website-claims not substantiated)
* 6 — dump collection failed
"""
from __future__ import annotations

import argparse
import dataclasses
import os
import pathlib
import sys
from typing import Optional, TextIO

from . import __version__
from . import audit as audit_mod
from . import boot_log as bl
from . import dump as dump_mod
from . import render
from . import report as report_mod
from . import trust_log as tl
from . import verify as verify_mod

EXIT_OK = 0
EXIT_USAGE = 1
EXIT_IO = 2
EXIT_PARSE = 3
EXIT_SCHEMA = 4
EXIT_VERIFY = 5
EXIT_DUMP = 6


# ---------------------------------------------------------------------------
# Default path resolution
# ---------------------------------------------------------------------------

DEFAULT_ESP_MOUNTS = (
    pathlib.Path("/boot/efi"),
    pathlib.Path("/efi"),
    pathlib.Path("/boot"),
)

DEFAULT_PATHS = {
    "trust_log": "loader/boot-trust.log",
    "boot_log": "EFI/LamBoot/reports/boot.log",
    "boot_json": "EFI/LamBoot/reports/boot.json",
    "audit_log": "EFI/LamBoot/reports/audit.log",
    "policy": "EFI/LamBoot/policy.toml",
}


def _find_esp() -> Optional[pathlib.Path]:
    for candidate in DEFAULT_ESP_MOUNTS:
        if candidate.is_dir() and (candidate / "EFI").exists():
            return candidate
    return None


def _resolve_default(key: str, explicit: Optional[str]) -> Optional[pathlib.Path]:
    if explicit:
        return pathlib.Path(explicit)
    esp = _find_esp()
    if esp is None:
        return None
    return esp / DEFAULT_PATHS[key]


# ---------------------------------------------------------------------------
# Subcommand handlers
# ---------------------------------------------------------------------------


def cmd_trust_log(args: argparse.Namespace) -> int:
    path = _resolve_default("trust_log", args.path)
    if path is None or not path.exists():
        print(
            f"lamboot-inspect: trust log not found (tried {path or 'ESP auto-detect'})",
            file=sys.stderr,
        )
        return EXIT_IO
    try:
        result = tl.parse(path)
    except (OSError, ValueError) as e:
        print(f"lamboot-inspect: cannot read trust log {path}: {e}", file=sys.stderr)
        return EXIT_IO

    if args.format == "json":
        render.render_trust_log_json(result, sys.stdout)
    elif args.format == "timeline":
        render.render_trust_log_timeline(result, sys.stdout)
    elif args.format == "stats":
        stats = tl.summarize(result)
        render.render_stats(stats, sys.stdout)
    else:
        render.render_trust_log_text(
            result,
            sys.stdout,
            filter_event=args.event,
            show_errors_only=args.errors_only,
            show_sha=not args.no_sha,
        )

    if result.schema_violations and args.strict:
        return EXIT_SCHEMA
    return EXIT_OK


def cmd_boot_log(args: argparse.Namespace) -> int:
    path = _resolve_default("boot_log", args.path)
    if path is None or not path.exists():
        print(
            f"lamboot-inspect: boot log not found (tried {path or 'ESP auto-detect'})",
            file=sys.stderr,
        )
        return EXIT_IO
    try:
        parsed = bl.parse(path)
    except (OSError, ValueError) as e:
        print(f"lamboot-inspect: cannot read boot log {path}: {e}", file=sys.stderr)
        return EXIT_IO

    level_filter: Optional[bl.Level] = None
    if args.level:
        try:
            level_filter = bl.Level(args.level.upper())
        except ValueError:
            print(
                f"lamboot-inspect: unknown level '{args.level}' "
                f"(valid: DEBUG, INFO, WARN, ERROR)",
                file=sys.stderr,
            )
            return EXIT_USAGE

    if args.format == "json":
        render.render_boot_log_json(parsed, sys.stdout)
    else:
        render.render_boot_log_text(
            parsed,
            sys.stdout,
            filter_level=level_filter,
            show_errors_only=args.errors_only,
        )
    return EXIT_OK


def cmd_summary(args: argparse.Namespace) -> int:
    trust_path = _resolve_default("trust_log", args.trust_path)
    boot_path = _resolve_default("boot_log", args.boot_path)
    report_path = _resolve_default("boot_json", args.report_path)
    audit_path = _resolve_default("audit_log", args.audit_path)

    trust_result: Optional[tl.ParseResult] = None
    if trust_path and trust_path.exists():
        try:
            trust_result = tl.parse(trust_path)
        except (OSError, ValueError) as e:
            print(f"lamboot-inspect: trust log parse failed: {e}", file=sys.stderr)

    boot_result: Optional[bl.ParsedBootLog] = None
    if boot_path and boot_path.exists():
        try:
            boot_result = bl.parse(boot_path)
        except (OSError, ValueError) as e:
            print(f"lamboot-inspect: boot log parse failed: {e}", file=sys.stderr)

    report_result: Optional[report_mod.BootReport] = None
    if report_path and report_path.exists():
        try:
            report_result = report_mod.parse(report_path)
        except (OSError, ValueError) as e:
            print(f"lamboot-inspect: boot report parse failed: {e}", file=sys.stderr)

    audit_result: Optional[audit_mod.ParsedAudit] = None
    if audit_path and audit_path.exists():
        try:
            audit_result = audit_mod.parse(audit_path)
        except (OSError, ValueError) as e:
            print(f"lamboot-inspect: audit log parse failed: {e}", file=sys.stderr)

    if (
        trust_result is None
        and boot_result is None
        and report_result is None
        and audit_result is None
    ):
        print("lamboot-inspect: no diagnostic artefacts found", file=sys.stderr)
        return EXIT_IO

    render.render_summary_text(
        trust_result, boot_result, report_result, audit_result, sys.stdout
    )
    return EXIT_OK


def cmd_verify(args: argparse.Namespace) -> int:
    repo_root = pathlib.Path(args.repo or ".").resolve()
    if not (repo_root / "lamboot-core").is_dir():
        print(
            f"lamboot-inspect: '{repo_root}' does not look like a lamboot-dev checkout "
            "(no lamboot-core subdirectory)",
            file=sys.stderr,
        )
        return EXIT_USAGE
    results = verify_mod.check_all(repo_root)
    all_ok = True
    for r in results:
        marker = "[OK]" if r.substantiated else "[FAIL]"
        print(f"{marker} {r.claim.name}: {r.claim.title}")
        if not r.substantiated:
            all_ok = False
            for ev in r.missing:
                print(f"        missing evidence: {ev.path} does not match /{ev.anchor_regex}/")
                if ev.description:
                    print(f"          ({ev.description})")
        if args.verbose:
            print(f"        spec: {r.claim.spec_section}")
    return EXIT_OK if all_ok else EXIT_VERIFY


def cmd_dump(args: argparse.Namespace) -> int:
    if args.output:
        out_path = pathlib.Path(args.output).resolve()
    else:
        import datetime as dt

        ts = dt.datetime.now().strftime("%Y%m%d-%H%M%S")
        out_path = pathlib.Path.cwd() / f"lamboot-inspect-dump-{ts}.tar.gz"

    esp_override = pathlib.Path(args.esp).resolve() if args.esp else None
    try:
        log = dump_mod.run(out_path, esp_override)
    except OSError as e:
        print(f"lamboot-inspect: dump failed: {e}", file=sys.stderr)
        return EXIT_DUMP

    print(f"lamboot-inspect: wrote {out_path}")
    print(f"  collected: {len(log.collected)} artefact(s)")
    if log.skipped:
        print(f"  skipped:   {len(log.skipped)} item(s) — see collection.log in bundle")
    if args.print_manifest:
        for c in log.collected:
            print(f"  {c.sha256}  {c.archive_path}")
    return EXIT_OK


def cmd_show(args: argparse.Namespace) -> int:
    path = _resolve_default("trust_log", args.path)
    if path is None or not path.exists():
        print(
            f"lamboot-inspect: trust log not found (tried {path or 'ESP auto-detect'})",
            file=sys.stderr,
        )
        return EXIT_IO
    result = tl.parse(path)
    for e in result.events:
        match = (
            str(e.seq) == args.event_id
            or e.event == args.event_id
            or (e.path and e.path == args.event_id)
        )
        if match:
            _print_event_detail(e, sys.stdout)
            return EXIT_OK
    print(f"lamboot-inspect: no event matching '{args.event_id}'", file=sys.stderr)
    return EXIT_IO


def _print_event_detail(event: tl.TrustEvent, stream: TextIO) -> None:
    stream.write(f"seq:           #{event.seq}\n")
    stream.write(f"event:         {event.event}\n")
    stream.write(f"classification: {event.classification()}\n")
    stream.write(f"path:          {event.path or '—'}\n")
    stream.write(f"size:          {event.size}\n")
    stream.write(f"sha256:        {event.sha256 or '—'}\n")
    stream.write(f"verified_via:  {event.verified_via or '—'}\n")
    stream.write(f"verifier_tag:  {event.verifier_tag or '—'}\n")
    stream.write(f"status:        {event.status or '—'}\n")
    if event.note:
        stream.write("note:\n")
        for line in event.note.splitlines():
            stream.write(f"  {line}\n")
    if event.extra:
        stream.write("extra:\n")
        for key, value in event.extra.items():
            stream.write(f"  {key}: {value}\n")


# ---------------------------------------------------------------------------
# Argument parser wiring
# ---------------------------------------------------------------------------


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="lamboot-inspect",
        description="LamBoot diagnostic tool — parse trust logs, boot logs, "
        "verify claims, and produce bug-report bundles.",
        epilog="Source and docs: https://github.com/lamco-admin/lamboot",
    )
    parser.add_argument("--version", action="version", version=f"lamboot-inspect {__version__}")
    sub = parser.add_subparsers(dest="command", required=True, metavar="<subcommand>")

    # trust-log
    p_trust = sub.add_parser(
        "trust-log",
        help="Parse and display the LamBoot trust log",
        description=(
            "Parses /loader/boot-trust.log (or an explicit --path) and prints "
            "events in text, JSON, timeline, or stats form."
        ),
    )
    p_trust.add_argument("-p", "--path", help="Path to the trust log file")
    p_trust.add_argument(
        "-f",
        "--format",
        choices=("text", "json", "timeline", "stats"),
        default="text",
        help="Output format (default: text)",
    )
    p_trust.add_argument("-e", "--event", help="Filter by event name")
    p_trust.add_argument("--errors-only", action="store_true", help="Show only failures/warnings")
    p_trust.add_argument("--no-sha", action="store_true", help="Omit sha256 digest columns")
    p_trust.add_argument(
        "--strict",
        action="store_true",
        help="Return exit code 4 if any schema violations are found",
    )
    p_trust.set_defaults(func=cmd_trust_log)

    # boot-log
    p_boot = sub.add_parser(
        "boot-log",
        help="Parse and display the LamBoot boot log (human-readable)",
    )
    p_boot.add_argument("-p", "--path", help="Path to boot.log")
    p_boot.add_argument("-l", "--level", help="Filter by level (DEBUG/INFO/WARN/ERROR)")
    p_boot.add_argument("--errors-only", action="store_true", help="Show only warnings/errors")
    p_boot.add_argument(
        "-f",
        "--format",
        choices=("text", "json"),
        default="text",
        help="Output format (default: text)",
    )
    p_boot.set_defaults(func=cmd_boot_log)

    # summary
    p_sum = sub.add_parser(
        "summary",
        help="One-page summary of the last boot (reads all available artefacts)",
    )
    p_sum.add_argument("--trust-path", help="Override trust log path")
    p_sum.add_argument("--boot-path", help="Override boot.log path")
    p_sum.add_argument("--report-path", help="Override boot.json path")
    p_sum.add_argument("--audit-path", help="Override audit.log path")
    p_sum.set_defaults(func=cmd_summary)

    # verify
    p_ver = sub.add_parser(
        "verify",
        help="Verify LamBoot website-claims are backed by code (SDS-4 §8.1)",
    )
    p_ver.add_argument(
        "--repo",
        help="Path to lamboot-dev checkout (default: current directory)",
    )
    p_ver.add_argument("-v", "--verbose", action="store_true", help="Include SDS spec references")
    p_ver.set_defaults(func=cmd_verify)

    # dump
    p_dump = sub.add_parser(
        "dump",
        help="Produce a diagnostic bundle for bug reports (tar.gz)",
    )
    p_dump.add_argument("-o", "--output", help="Output bundle path")
    p_dump.add_argument("--esp", help="ESP mount point (override auto-detection)")
    p_dump.add_argument(
        "--print-manifest",
        action="store_true",
        help="Print SHA-256 manifest of collected files after bundling",
    )
    p_dump.set_defaults(func=cmd_dump)

    # show
    p_show = sub.add_parser(
        "show",
        help="Show a single trust-log event in detail (by seq, event name, or path)",
    )
    p_show.add_argument("event_id", help="Sequence number, event name, or image path")
    p_show.add_argument("-p", "--path", help="Override trust log path")
    p_show.set_defaults(func=cmd_show)

    return parser


def main(argv: "Optional[list]" = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
