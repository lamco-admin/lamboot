"""End-to-end tests for `lamboot-inspect diagnose` and the harmonized contracts.

These exercise the entry-point via subprocess, asserting the shared toolkit
exit codes (0 clean / 0+warn / 2 problems / 4 strict-integrity), the JSON
envelope, and the diagnose rules engine against the trust-log fixtures.
"""
from __future__ import annotations

import json
import os
import pathlib
import subprocess
import sys

HERE = pathlib.Path(__file__).resolve()
FIXTURES = HERE.parent / "fixtures"
SCRIPT = HERE.parent.parent / "lamboot-inspect"


def run(*args: str) -> subprocess.CompletedProcess:
    env = {**os.environ, "NO_COLOR": "1"}
    return subprocess.run(
        [sys.executable, str(SCRIPT), *args],
        capture_output=True, text=True, env=env, check=False,
    )


def test_diagnose_clean_fixture_exits_ok():
    r = run("diagnose", "-p", str(FIXTURES / "trust-log.jsonl"))
    assert r.returncode == 0
    assert "clean" in r.stdout


def test_diagnose_failed_fixture_exits_partial():
    r = run("diagnose", "-p", str(FIXTURES / "trust-log-failed.jsonl"))
    # Error findings -> EXIT_PARTIAL(2), matching the suite's diagnostic convention.
    assert r.returncode == 2
    assert "boot.event_failed" in r.stdout


def test_diagnose_json_envelope_on_failed_boot():
    r = run("diagnose", "-p", str(FIXTURES / "trust-log-failed.jsonl"), "--json")
    assert r.returncode == 2
    obj = json.loads(r.stdout)
    assert obj["schema_version"] == "v1"
    assert obj["tool"] == "lamboot-inspect"
    assert obj["summary"]["status"] == "fail"
    assert obj["exit_code"] == 2
    assert any(f["id"] == "boot.event_failed" for f in obj["findings"])


def test_diagnose_clean_json_status_pass():
    r = run("diagnose", "-p", str(FIXTURES / "trust-log.jsonl"), "--json")
    assert r.returncode == 0
    assert json.loads(r.stdout)["summary"]["status"] == "pass"


def test_diagnose_severity_floor_filters_info():
    # The failed fixture also carries a warning (crashloop); --severity error
    # should drop info/warn from the report but still exit 2 on the errors.
    r = run("diagnose", "-p", str(FIXTURES / "trust-log-failed.jsonl"), "--severity", "error")
    assert r.returncode == 2
    assert "crashloop" not in r.stdout
    assert "boot.event_failed" in r.stdout


def test_json_schema_flag_exits_ok():
    r = run("--json-schema")
    assert r.returncode == 0
    assert json.loads(r.stdout)["title"] == "lamboot-tools JSON envelope"


def test_dump_registry_is_bash_compatible():
    r = run("--dump-registry")
    assert r.returncode == 0
    assert "TOOL_NAME=lamboot-inspect" in r.stdout
    assert "---REGISTRY-BEGIN---" in r.stdout
    assert "---REGISTRY-END---" in r.stdout
    assert "diagnose" in r.stdout
    # 13-field 0x1F records.
    body = r.stdout.split("---REGISTRY-BEGIN---", 1)[1].split("---REGISTRY-END---", 1)[0]
    record = next(line for line in body.splitlines() if line.strip())
    assert record.count("\x1f") == 12


def test_no_subcommand_exits_error():
    r = run()
    assert r.returncode == 1
