"""Unit tests for the shared JSON envelope.

The envelope must stay byte-compatible with the bash suite's `emit_json`
(SPEC-LAMBOOT-TOOLKIT-V1 §5): same top-level keys in the same order, the same
`summary.status` derived from the exit code, and the same `findings_by_severity`
shape — so a fleet aggregator reads one schema across bash and Python tools.
"""
from __future__ import annotations

import datetime as dt
import pathlib
import sys

HERE = pathlib.Path(__file__).resolve()
sys.path.insert(0, str(HERE.parent.parent))

from lamboot_inspect import envelope as env  # noqa: E402


def test_status_for_exit_matches_bash_mapping():
    # Mirrors lib/lamboot-toolkit-lib.sh :: emit_json aggregate_status case.
    assert env.status_for_exit(0, 0) == "pass"
    assert env.status_for_exit(0, 3) == "warn"
    assert env.status_for_exit(2, 0) == "fail"
    assert env.status_for_exit(3, 0) == "noop"
    assert env.status_for_exit(4, 0) == "unsafe"
    assert env.status_for_exit(5, 0) == "abort"
    assert env.status_for_exit(1, 0) == "error"
    assert env.status_for_exit(7, 0) == "error"


def test_build_has_suite_envelope_keys_in_order():
    e = env.build(
        tool="lamboot-inspect", version="0.0.0", command="lamboot-inspect",
        exit_code=0, findings=[], toolkit_version="0.8.2", host="h", run_id="r",
    )
    assert list(e.keys()) == [
        "schema_version", "tool", "version", "toolkit_version", "timestamp",
        "host", "run_id", "command", "dry_run", "exit_code", "summary",
        "findings", "actions_taken", "backup_dir",
    ]
    assert e["schema_version"] == "v1"
    assert e["summary"]["status"] == "pass"
    assert e["summary"]["findings_by_severity"] == {
        "critical": 0, "error": 0, "warning": 0, "info": 0,
    }
    assert e["findings"] == []
    assert e["actions_taken"] == []
    assert e["backup_dir"] is None
    assert "data" not in e


def test_status_and_counts_track_findings_and_exit():
    f = env.Finding(id="x", category="c", severity=env.SEV_WARNING, status="warn", title="t")
    e = env.build(
        tool="t", version="0", command="t", exit_code=0, findings=[f],
        toolkit_version="0.8.2", host="h", run_id="r",
    )
    assert e["summary"]["findings_by_severity"]["warning"] == 1
    # exit 0 + a warning -> "warn" (warnings are non-fatal in the suite).
    assert e["summary"]["status"] == "warn"


def test_data_block_is_additive():
    e = env.build(
        tool="t", version="0", command="t", exit_code=0, findings=[], data={"k": 1},
        toolkit_version="0.8.2", host="h", run_id="r",
    )
    assert e["data"] == {"k": 1}


def test_finding_to_dict_shape():
    f = env.Finding(
        id="i", category="c", severity=env.SEV_ERROR, status="fail", title="T",
        message="m", context={"a": 1}, remediation=env.remediation("do x", "cmd", "url"),
    )
    d = f.to_dict()
    assert list(d.keys()) == ["id", "category", "severity", "status", "title", "message", "context", "remediation"]
    assert d["remediation"] == {"summary": "do x", "command": "cmd", "doc_url": "url"}


def test_make_run_id_format():
    rid = env.make_run_id(dt.datetime(2026, 6, 8, 1, 2, 3), "abcdef99")
    assert rid == "2026-06-08T01-02-03-abcdef"


def test_schema_declares_required_envelope_fields():
    s = env.schema()
    assert s["title"] == "lamboot-tools JSON envelope"
    assert "summary" in s["properties"]
    assert "findings" in s["properties"]
