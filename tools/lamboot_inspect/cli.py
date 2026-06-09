"""Command-line interface.

All subcommand dispatch and argument handling lives here. The individual
subcommand handlers are small adapters over the parser / renderer modules —
keeping CLI policy out of the data code and making every subcommand
independently testable.

`lamboot-inspect` is the suite's one Python tool; it presents identically to its
bash siblings (`SPEC-LAMBOOT-TOOLKIT-V1`):

* Exit codes are the shared toolkit table (EXIT_OK..EXIT_PREREQUISITE), NOT a
  tool-local scheme.
* `--json` emits the shared envelope (see :mod:`envelope`); `--json-schema`
  prints the envelope schema; universal flags (`--no-color`, `-q/--quiet`,
  `-v/--verbose`) are accepted on every subcommand.
* Subcommand help is declared once in :mod:`registry` and re-emitted in the
  bash-compatible dump format via the hidden `--dump-registry`, so man/website
  generation is uniform across bash and Python tools.
"""
from __future__ import annotations

import argparse
import pathlib
import sys
from typing import Optional, TextIO

from . import __version__
from . import audit as audit_mod
from . import audit_ext
from . import boot_log as bl
from . import diagnose as diagnose_mod
from . import dump as dump_mod
from . import envelope as env_mod
from . import registry as registry_mod
from . import render
from . import report as report_mod
from . import trust_log as tl
from . import verify as verify_mod

# Shared toolkit exit codes (SPEC-LAMBOOT-TOOLKIT-V1). A consumer scripting the
# suite gets the same semantics from every tool, bash or Python.
EXIT_OK = 0
EXIT_ERROR = 1
EXIT_PARTIAL = 2
EXIT_NOOP = 3
EXIT_UNSAFE = 4
EXIT_ABORT = 5
EXIT_NOT_APPLICABLE = 6
EXIT_PREREQUISITE = 7

TOOL_NAME = "lamboot-inspect"


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
        try:
            if candidate.is_dir() and (candidate / "EFI").exists():
                return candidate
        except OSError:
            # ESP is typically root-only; an unreadable candidate is "not it".
            continue
    return None


def _resolve_default(key: str, explicit: Optional[str]) -> Optional[pathlib.Path]:
    if explicit:
        return pathlib.Path(explicit)
    esp = _find_esp()
    if esp is None:
        return None
    return esp / DEFAULT_PATHS[key]


# ---------------------------------------------------------------------------
# Envelope helpers
# ---------------------------------------------------------------------------


def _emit(args: argparse.Namespace, exit_code: int, findings, data: Optional[dict]) -> int:
    """Emit the shared JSON envelope and return the exit code.

    Called by a subcommand only on its `--json` path; the text path renders
    directly and returns the code itself.
    """
    envelope = env_mod.build(
        tool=TOOL_NAME,
        version=__version__,
        command=TOOL_NAME,
        exit_code=exit_code,
        findings=findings,
        data=data,
    )
    env_mod.emit(envelope, sys.stdout)
    return exit_code


def _io_error(args: argparse.Namespace, message: str) -> int:
    """Report a missing/unreadable artefact uniformly on both output paths."""
    if getattr(args, "json", False):
        f = env_mod.Finding(
            id="inspect.artefact_missing",
            category="inspect",
            severity=env_mod.SEV_ERROR,
            status="fail",
            title="Diagnostic artefact not found",
            message=message,
        )
        return _emit(args, EXIT_ERROR, [f], None)
    print(f"lamboot-inspect: {message}", file=sys.stderr)
    return EXIT_ERROR


def _schema_findings(result: "tl.ParseResult") -> "list[env_mod.Finding]":
    """Findings about the LOG ITSELF (integrity), not the boot's health.

    `trust-log` reports these; the boot *verdict* (is this boot healthy?) is the
    job of `diagnose`/`summary`, which run the rules engine. Keeping the two
    separate avoids `trust-log` re-judging a degraded-but-expected event (e.g.
    image_verified status=SKIPPED on a Secure-Boot-off boot) as a failure.
    """
    return [
        env_mod.Finding(
            id="trust.schema_violation",
            category="trust",
            severity=env_mod.SEV_WARNING,
            status="warn",
            title="Trust-log schema violation",
            message="; ".join(sv.issues),
            context={"line": sv.line_number},
        )
        for sv in result.schema_violations
    ]


# ---------------------------------------------------------------------------
# Subcommand handlers
# ---------------------------------------------------------------------------


def cmd_trust_log(args: argparse.Namespace) -> int:
    path = _resolve_default("trust_log", args.path)
    if path is None or not path.exists():
        return _io_error(args, f"trust log not found (tried {path or 'ESP auto-detect'})")
    try:
        result = tl.parse(path)
    except (OSError, ValueError) as e:
        return _io_error(args, f"cannot read trust log {path}: {e}")

    exit_code = EXIT_OK
    if result.schema_violations and args.strict:
        exit_code = EXIT_UNSAFE

    if args.json:
        stats = tl.summarize(result)
        data = {
            "source_path": str(result.source_path) if result.source_path else None,
            "source_sha256": result.source_sha256,
            "event_count": stats.total_events,
            "by_event": stats.by_event,
            "by_verified_via": stats.by_verified_via,
            "sha256_verify_vs_load_match": stats.sha256_verify_vs_load_match,
        }
        return _emit(args, exit_code, _schema_findings(result), data)

    if args.format == "json":
        render.render_trust_log_json(result, sys.stdout)
    elif args.format == "timeline":
        render.render_trust_log_timeline(result, sys.stdout)
    elif args.format == "stats":
        render.render_stats(tl.summarize(result), sys.stdout)
    else:
        render.render_trust_log_text(
            result,
            sys.stdout,
            filter_event=args.event,
            show_errors_only=args.errors_only,
            show_sha=not args.no_sha,
        )
    return exit_code


def cmd_boot_log(args: argparse.Namespace) -> int:
    path = _resolve_default("boot_log", args.path)
    if path is None or not path.exists():
        return _io_error(args, f"boot log not found (tried {path or 'ESP auto-detect'})")
    try:
        parsed = bl.parse(path)
    except (OSError, ValueError) as e:
        return _io_error(args, f"cannot read boot log {path}: {e}")

    level_filter: Optional[bl.Level] = None
    if args.level:
        try:
            level_filter = bl.Level(args.level.upper())
        except ValueError:
            return _io_error(
                args, f"unknown level '{args.level}' (valid: DEBUG, INFO, WARN, ERROR)"
            )

    if args.json:
        errors = parsed.errors()
        findings = [
            env_mod.Finding(
                id="boot.log_problem",
                category="boot",
                severity=(
                    env_mod.SEV_ERROR if e.level == bl.Level.ERROR else env_mod.SEV_WARNING
                ),
                status="fail" if e.level == bl.Level.ERROR else "warn",
                title=f"{e.level.value}: boot-log entry",
                message=getattr(e, "message", ""),
            )
            for e in errors
        ]
        data = {"entry_count": len(parsed.entries), "problem_count": len(errors)}
        return _emit(args, EXIT_OK, findings, data)

    if args.format == "json":
        render.render_boot_log_json(parsed, sys.stdout)
    else:
        render.render_boot_log_text(
            parsed, sys.stdout, filter_level=level_filter, show_errors_only=args.errors_only
        )
    return EXIT_OK


def cmd_summary(args: argparse.Namespace) -> int:
    trust_path = _resolve_default("trust_log", args.trust_path)
    boot_path = _resolve_default("boot_log", args.boot_path)
    report_path = _resolve_default("boot_json", args.report_path)
    audit_path = _resolve_default("audit_log", args.audit_path)

    trust_result = None
    if trust_path and trust_path.exists():
        try:
            trust_result = tl.parse(trust_path)
        except (OSError, ValueError) as e:
            print(f"lamboot-inspect: trust log parse failed: {e}", file=sys.stderr)

    boot_result = None
    if boot_path and boot_path.exists():
        try:
            boot_result = bl.parse(boot_path)
        except (OSError, ValueError) as e:
            print(f"lamboot-inspect: boot log parse failed: {e}", file=sys.stderr)

    report_result = None
    if report_path and report_path.exists():
        try:
            report_result = report_mod.parse(report_path)
        except (OSError, ValueError) as e:
            print(f"lamboot-inspect: boot report parse failed: {e}", file=sys.stderr)

    audit_result = None
    if audit_path and audit_path.exists():
        try:
            audit_result = audit_mod.parse(audit_path)
        except (OSError, ValueError) as e:
            print(f"lamboot-inspect: audit log parse failed: {e}", file=sys.stderr)

    if trust_result is None and boot_result is None and report_result is None and audit_result is None:
        return _io_error(args, "no diagnostic artefacts found")

    if args.json:
        findings = []
        if trust_result is not None:
            ctx = diagnose_mod.Ctx(trust_result, tl.summarize(trust_result))
            findings = diagnose_mod.evaluate(ctx) + _schema_findings(trust_result)
        data = {"artefacts_present": {
            "trust_log": trust_result is not None,
            "boot_log": boot_result is not None,
            "boot_json": report_result is not None,
            "audit_log": audit_result is not None,
        }}
        return _emit(args, EXIT_OK, findings, data)

    render.render_summary_text(trust_result, boot_result, report_result, audit_result, sys.stdout)
    return EXIT_OK


def cmd_verify(args: argparse.Namespace) -> int:
    repo_root = pathlib.Path(args.repo or ".").resolve()
    if not (repo_root / "lamboot-core").is_dir():
        return _io_error(
            args,
            f"'{repo_root}' does not look like a lamboot-dev checkout (no lamboot-core subdirectory)",
        )
    results = verify_mod.check_all(repo_root)
    all_ok = all(r.substantiated for r in results)

    if args.json:
        findings = [
            env_mod.Finding(
                id=f"claim.{r.claim.name}",
                category="claim",
                severity=env_mod.SEV_INFO if r.substantiated else env_mod.SEV_ERROR,
                status="pass" if r.substantiated else "fail",
                title=r.claim.title,
                message="" if r.substantiated else "claim not substantiated by code",
                context={"spec_section": r.claim.spec_section},
            )
            for r in results
        ]
        return _emit(args, EXIT_OK if all_ok else EXIT_PARTIAL, findings, None)

    for r in results:
        marker = "[OK]" if r.substantiated else "[FAIL]"
        print(f"{marker} {r.claim.name}: {r.claim.title}")
        if not r.substantiated:
            for ev in r.missing:
                print(f"        missing evidence: {ev.path} does not match /{ev.anchor_regex}/")
                if ev.description:
                    print(f"          ({ev.description})")
        if args.verbose:
            print(f"        spec: {r.claim.spec_section}")
    return EXIT_OK if all_ok else EXIT_PARTIAL


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
        return _io_error(args, f"dump failed: {e}")

    if args.json:
        data = {
            "bundle": str(out_path),
            "collected": len(log.collected),
            "skipped": len(log.skipped),
        }
        return _emit(args, EXIT_OK, [], data)

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
        return _io_error(args, f"trust log not found (tried {path or 'ESP auto-detect'})")
    result = tl.parse(path)
    for e in result.events:
        if (
            str(e.seq) == args.event_id
            or e.event == args.event_id
            or (e.path and e.path == args.event_id)
        ):
            if args.json:
                data = {
                    "seq": e.seq, "event": e.event, "classification": e.classification(),
                    "path": e.path, "size": e.size, "sha256": e.sha256,
                    "verified_via": e.verified_via, "status": e.status, "note": e.note,
                    "extra": e.extra,
                }
                return _emit(args, EXIT_OK, [], data)
            _print_event_detail(e, sys.stdout)
            return EXIT_OK
    return _io_error(args, f"no event matching '{args.event_id}'")


def cmd_diagnose(args: argparse.Namespace) -> int:
    return diagnose_mod.run(args, _resolve_default, _emit, _io_error)


def cmd_history(args: argparse.Namespace) -> int:
    return audit_ext.run_history(args, _resolve_default, _emit, _io_error)


def cmd_diff(args: argparse.Namespace) -> int:
    return audit_ext.run_diff(args, _resolve_default, _emit, _io_error)


def cmd_attest(args: argparse.Namespace) -> int:
    return audit_ext.run_attest(args, _resolve_default, _emit, _io_error)


def _print_event_detail(event: "tl.TrustEvent", stream: TextIO) -> None:
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


def _common_parent() -> argparse.ArgumentParser:
    """Universal flags shared by every subcommand (suite convention)."""
    common = argparse.ArgumentParser(add_help=False)
    common.add_argument("--json", action="store_true", help="Emit the shared toolkit JSON envelope")
    common.add_argument("--no-color", action="store_true", help="Disable ANSI color")
    common.add_argument("-q", "--quiet", action="store_true", help="Only warnings and errors")
    common.add_argument("-v", "--verbose", action="store_true", help="Extra informational output")
    return common


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog=TOOL_NAME,
        description="LamBoot diagnostic tool — parse trust/boot logs, diagnose the "
        "last boot, verify claims, and produce bug-report bundles.",
        epilog="Source and docs: https://github.com/lamco-admin/lamboot",
    )
    parser.add_argument("--version", action="version", version=f"{TOOL_NAME} {__version__}")
    parser.add_argument("--json-schema", action="store_true", help="Print the JSON envelope schema and exit")
    parser.add_argument("--dump-registry", action="store_true", help=argparse.SUPPRESS)
    common = _common_parent()
    sub = parser.add_subparsers(dest="command", required=False, metavar="<subcommand>")

    def _help(name: str) -> str:
        sc = registry_mod.find(name)
        return sc.summary if sc else ""

    p_trust = sub.add_parser("trust-log", parents=[common], help=_help("trust-log"))
    p_trust.add_argument("-p", "--path", help="Path to the trust log file")
    p_trust.add_argument("-f", "--format", choices=("text", "json", "timeline", "stats"), default="text")
    p_trust.add_argument("-e", "--event", help="Filter by event name")
    p_trust.add_argument("--errors-only", action="store_true")
    p_trust.add_argument("--no-sha", action="store_true")
    p_trust.add_argument("--strict", action="store_true")
    p_trust.set_defaults(func=cmd_trust_log)

    p_boot = sub.add_parser("boot-log", parents=[common], help=_help("boot-log"))
    p_boot.add_argument("-p", "--path")
    p_boot.add_argument("-l", "--level")
    p_boot.add_argument("--errors-only", action="store_true")
    p_boot.add_argument("-f", "--format", choices=("text", "json"), default="text")
    p_boot.set_defaults(func=cmd_boot_log)

    p_sum = sub.add_parser("summary", parents=[common], help=_help("summary"))
    p_sum.add_argument("--trust-path")
    p_sum.add_argument("--boot-path")
    p_sum.add_argument("--report-path")
    p_sum.add_argument("--audit-path")
    p_sum.set_defaults(func=cmd_summary)

    p_diag = sub.add_parser("diagnose", parents=[common], help=_help("diagnose"))
    p_diag.add_argument("-p", "--path", help="Trust log path")
    p_diag.add_argument("--boot-path", help="boot.json path")
    p_diag.add_argument(
        "-s", "--severity", choices=("info", "warn", "error"), default="info",
        help="Minimum severity to report (default: info)",
    )
    p_diag.add_argument("--explain", action="store_true", help="Show the evidence each finding derives from")
    p_diag.add_argument("--strict", action="store_true", help="Integrity violation -> exit UNSAFE (4)")
    p_diag.add_argument("--format", choices=("text", "json"), default="text")
    p_diag.set_defaults(func=cmd_diagnose)

    p_hist = sub.add_parser("history", parents=[common], help=_help("history"))
    p_hist.add_argument("-p", "--path", help="History directory")
    p_hist.add_argument("--limit", type=int, help="Show at most N past boots")
    p_hist.set_defaults(func=cmd_history)

    p_diff = sub.add_parser("diff", parents=[common], help=_help("diff"))
    p_diff.add_argument("--from", dest="from_", help="Baseline boot log")
    p_diff.add_argument("--to", help="Comparison boot log (default: current)")
    p_diff.set_defaults(func=cmd_diff)

    p_att = sub.add_parser("attest", parents=[common], help=_help("attest"))
    p_att.add_argument("-p", "--path", help="Trust log path")
    p_att.add_argument("--out", help="Write the attestation document to FILE")
    p_att.set_defaults(func=cmd_attest)

    p_ver = sub.add_parser("verify", parents=[common], help=_help("verify"))
    p_ver.add_argument("--repo")
    p_ver.set_defaults(func=cmd_verify)

    p_dump = sub.add_parser("dump", parents=[common], help=_help("dump"))
    p_dump.add_argument("-o", "--output")
    p_dump.add_argument("--esp")
    p_dump.add_argument("--print-manifest", action="store_true")
    p_dump.set_defaults(func=cmd_dump)

    p_show = sub.add_parser("show", parents=[common], help=_help("show"))
    p_show.add_argument("event_id")
    p_show.add_argument("-p", "--path")
    p_show.set_defaults(func=cmd_show)

    return parser


def main(argv: "Optional[list]" = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)

    if getattr(args, "json_schema", False):
        env_mod.emit(env_mod.schema(), sys.stdout)
        return EXIT_OK
    if getattr(args, "dump_registry", False):
        sys.stdout.write(
            registry_mod.dump(TOOL_NAME, __version__, env_mod._toolkit_version())
        )
        return EXIT_OK
    if not getattr(args, "command", None):
        parser.print_help(sys.stderr)
        return EXIT_ERROR
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
