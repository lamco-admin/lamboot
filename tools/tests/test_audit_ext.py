"""Tests for the audit-grade inspect features: history, diff, attest."""
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
    return subprocess.run([sys.executable, str(SCRIPT), *args],
                          capture_output=True, text=True, env=env, check=False)


def test_attest_reports_measured_boot_and_sha():
    r = run("attest", "-p", str(FIXTURES / "trust-log.jsonl"), "--json")
    assert r.returncode == 0
    att = json.loads(r.stdout)["data"]["attestation"]
    assert att["verified_via"] == "shim_mok"
    assert att["loaded_sha256"]
    assert att["measured_boot"] == "available"


def test_attest_writes_out_file(tmp_path):
    out = tmp_path / "boot.attest.json"
    r = run("attest", "-p", str(FIXTURES / "trust-log.jsonl"), "--out", str(out))
    assert r.returncode == 0
    assert out.exists()
    assert json.loads(out.read_text())["loaded_sha256"]


def test_diff_flags_boot_regression():
    r = run("diff", "--from", str(FIXTURES / "trust-log.jsonl"),
            "--to", str(FIXTURES / "trust-log-failed.jsonl"), "--json")
    assert r.returncode == 2
    ids = [f["id"] for f in json.loads(r.stdout)["findings"]]
    assert "diff.boot_regressed" in ids


def test_diff_recovery_is_not_a_regression():
    r = run("diff", "--from", str(FIXTURES / "trust-log-failed.jsonl"),
            "--to", str(FIXTURES / "trust-log.jsonl"))
    assert r.returncode == 0


def test_diff_identical_is_clean():
    r = run("diff", "--from", str(FIXTURES / "trust-log.jsonl"),
            "--to", str(FIXTURES / "trust-log.jsonl"))
    assert r.returncode == 0
    assert "no regressions" in r.stdout


def test_history_without_dir_is_graceful():
    r = run("history", "-p", "/tmp/lamboot-no-such-history-xyz")
    assert r.returncode == 0
    assert "No retained boot history" in r.stdout
