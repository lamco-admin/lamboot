"""Toolkit JSON envelope — byte-compatible with the bash suite's `emit_json`.

`lamboot-inspect` is the suite's one Python tool. To keep it indistinguishable
from its bash siblings on the outside, every `--json` output uses the *same*
envelope the shared library emits (`lib/lamboot-toolkit-lib.sh` :: `emit_json`,
per `SPEC-LAMBOOT-TOOLKIT-V1` §5): the same top-level keys in the same order,
the same `findings[]` shape, the same `summary.status` vocabulary, and the same
`run_id`/`timestamp` formats. A fleet aggregator consuming both bash and Python
tool output sees one schema.

This module owns the envelope; it has no I/O policy of its own beyond a final
`emit()` that writes compact JSON to a stream.
"""
from __future__ import annotations

import dataclasses
import datetime as _dt
import json
import os
import pathlib
import socket
from typing import Optional, TextIO

# Mirrors LAMBOOT_TOOLKIT_SCHEMA_VERSION in lib/lamboot-toolkit-lib.sh.
SCHEMA_VERSION = "v1"

# Severity vocabulary (matches the bash lib's add_finding severities).
SEV_CRITICAL = "critical"
SEV_ERROR = "error"
SEV_WARNING = "warning"
SEV_INFO = "info"
_SEVERITY_ORDER = (SEV_CRITICAL, SEV_ERROR, SEV_WARNING, SEV_INFO)

# Aggregate status vocabulary (summary.status), per §5.2.
STATUS_PASS = "pass"
STATUS_WARN = "warn"
STATUS_FAIL = "fail"
STATUS_NOOP = "noop"
STATUS_ERROR = "error"
STATUS_UNSAFE = "unsafe"
STATUS_ABORT = "abort"


@dataclasses.dataclass
class Finding:
    """One finding — the same shape the bash lib's `add_finding` records.

    `severity` is one of the SEV_* constants; `status` is the per-finding
    outcome (`pass`/`warn`/`fail`/`info`/`skip`). `remediation` carries the
    optional `{summary, command, doc_url}` an operator can act on.
    """

    id: str
    category: str
    severity: str
    status: str
    title: str
    message: str = ""
    context: dict = dataclasses.field(default_factory=dict)
    remediation: Optional[dict] = None

    def to_dict(self) -> dict:
        return {
            "id": self.id,
            "category": self.category,
            "severity": self.severity,
            "status": self.status,
            "title": self.title,
            "message": self.message,
            "context": self.context,
            "remediation": self.remediation,
        }


def remediation(summary: str, command: str = "", doc_url: str = "") -> dict:
    return {"summary": summary, "command": command, "doc_url": doc_url}


def status_for_exit(exit_code: int, warning_count: int) -> str:
    """Map an exit code to `summary.status`, byte-identical to the bash lib's
    `emit_json` (`lib/lamboot-toolkit-lib.sh`): the status is a function of the
    exit code (plus the warning count for the success case), NOT of finding
    severity. Keeping this identical means a fleet aggregator reads one rule for
    bash and Python tools alike.
    """
    if exit_code == 0:
        return STATUS_WARN if warning_count > 0 else STATUS_PASS
    if exit_code == 2:
        return STATUS_FAIL
    if exit_code == 3:
        return STATUS_NOOP
    if exit_code == 4:
        return STATUS_UNSAFE
    if exit_code == 5:
        return STATUS_ABORT
    return STATUS_ERROR


def _toolkit_version() -> str:
    """Resolve the unified toolkit version.

    Prefer the env var the suite's tools share; fall back to reading the
    sibling lib in a source checkout; finally a bundled constant kept in sync
    at release time. The per-tool version is the package `__version__`.
    """
    env = os.environ.get("LAMBOOT_TOOLKIT_VERSION")
    if env:
        return env
    here = pathlib.Path(__file__).resolve()
    for parent in here.parents:
        lib = parent / "lib" / "lamboot-toolkit-lib.sh"
        if lib.is_file():
            try:
                for line in lib.read_text().splitlines():
                    if "LAMBOOT_TOOLKIT_VERSION=" in line:
                        return line.split("=", 1)[1].strip().strip('"')
            except OSError:
                break
    return "0.9.0"


def make_run_id(now: _dt.datetime, rand_hex: str) -> str:
    """`<ISO-with-dashes>-<6hex>` — matches the bash `LAMBOOT_RUN_ID` format.

    Time + randomness are injected so callers stay testable/deterministic.
    """
    stamp = now.strftime("%Y-%m-%dT%H-%M-%S")
    return f"{stamp}-{rand_hex[:6]}"


def build(
    *,
    tool: str,
    version: str,
    command: str,
    exit_code: int,
    findings: "list[Finding]",
    data: Optional[dict] = None,
    dry_run: bool = False,
    actions_taken: Optional[list] = None,
    backup_dir: Optional[str] = None,
    toolkit_version: Optional[str] = None,
    now: Optional[_dt.datetime] = None,
    run_id: Optional[str] = None,
    host: Optional[str] = None,
) -> dict:
    """Assemble the envelope dict with the suite's key order.

    `data` is an inspect-specific extension block (the parsed trust-log,
    boot-log, etc.). It is additive: bash tools omit it, so a consumer keys off
    `findings` for the common contract and `data` when it wants the detail.
    """
    now = now or _dt.datetime.now(_dt.timezone.utc)
    if run_id is None:
        run_id = make_run_id(now, os.urandom(3).hex())
    counts = {s: 0 for s in _SEVERITY_ORDER}
    for f in findings:
        if f.severity in counts:
            counts[f.severity] += 1
    env = {
        "schema_version": SCHEMA_VERSION,
        "tool": tool,
        "version": version,
        "toolkit_version": toolkit_version or _toolkit_version(),
        "timestamp": now.strftime("%Y-%m-%dT%H:%M:%SZ"),
        "host": host if host is not None else socket.gethostname(),
        "run_id": run_id,
        "command": command,
        "dry_run": dry_run,
        "exit_code": exit_code,
        "summary": {
            "status": status_for_exit(exit_code, counts[SEV_WARNING]),
            "findings_total": len(findings),
            "findings_by_severity": counts,
        },
        "findings": [f.to_dict() for f in findings],
        "actions_taken": actions_taken or [],
        "backup_dir": backup_dir,
    }
    if data is not None:
        env["data"] = data
    return env


def emit(envelope: dict, stream: TextIO) -> None:
    json.dump(envelope, stream, separators=(",", ":"))
    stream.write("\n")


def schema() -> dict:
    """The `--json-schema` payload: a description of the envelope this tool emits."""
    return {
        "schema_version": SCHEMA_VERSION,
        "title": "lamboot-tools JSON envelope",
        "type": "object",
        "required": [
            "schema_version",
            "tool",
            "version",
            "toolkit_version",
            "timestamp",
            "host",
            "run_id",
            "command",
            "dry_run",
            "exit_code",
            "summary",
            "findings",
        ],
        "properties": {
            "summary": {
                "type": "object",
                "properties": {
                    "status": {
                        "enum": [
                            STATUS_PASS,
                            STATUS_WARN,
                            STATUS_FAIL,
                            STATUS_NOOP,
                            STATUS_ERROR,
                            STATUS_UNSAFE,
                            STATUS_ABORT,
                        ]
                    },
                },
            },
            "findings": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["id", "category", "severity", "status", "title"],
                    "properties": {
                        "severity": {
                            "enum": [SEV_CRITICAL, SEV_ERROR, SEV_WARNING, SEV_INFO]
                        }
                    },
                },
            },
            "data": {"type": "object", "description": "inspect-specific detail block"},
        },
    }
