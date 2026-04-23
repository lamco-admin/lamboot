"""End-to-end CLI tests using subprocess.

Exercises the ``lamboot-inspect`` entry-point script with realistic
fixtures, checking exit codes and the shape of output.
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
        capture_output=True,
        text=True,
        env=env,
        check=False,
    )


def test_version():
    r = run("--version")
    assert r.returncode == 0
    assert "lamboot-inspect" in r.stdout


def test_trust_log_text_format():
    r = run("trust-log", "--path", str(FIXTURES / "trust-log.jsonl"))
    assert r.returncode == 0
    assert "image_verified" in r.stdout
    assert "shim_mok" in r.stdout


def test_trust_log_json_format_is_valid_json():
    r = run("trust-log", "-p", str(FIXTURES / "trust-log.jsonl"), "-f", "json")
    assert r.returncode == 0
    obj = json.loads(r.stdout)
    assert "events" in obj
    assert len(obj["events"]) == 13


def test_trust_log_errors_only_surfaces_only_failures():
    r = run(
        "trust-log",
        "-p",
        str(FIXTURES / "trust-log-failed.jsonl"),
        "--errors-only",
    )
    assert r.returncode == 0
    assert "image_load_failed" in r.stdout
    # boot_start on a healthy line should not appear
    assert "[OK]" not in r.stdout.replace("[FAIL]", "").replace("[WARN]", "")


def test_trust_log_stats_format():
    r = run("trust-log", "-p", str(FIXTURES / "trust-log.jsonl"), "-f", "stats")
    assert r.returncode == 0
    assert "Trust-log statistics" in r.stdout
    assert "Boot attempts" in r.stdout


def test_boot_log_text():
    r = run("boot-log", "-p", str(FIXTURES / "boot.log"))
    assert r.returncode == 0
    assert "LamBoot 0.8.3" in r.stdout


def test_boot_log_errors_only():
    r = run("boot-log", "-p", str(FIXTURES / "boot.log"), "--errors-only")
    assert r.returncode == 0
    assert "WARN" in r.stdout


def test_summary_pulls_multiple_artefacts():
    r = run(
        "summary",
        "--trust-path",
        str(FIXTURES / "trust-log.jsonl"),
        "--boot-path",
        str(FIXTURES / "boot.log"),
        "--report-path",
        str(FIXTURES / "boot.json"),
        "--audit-path",
        str(FIXTURES / "audit.log"),
    )
    assert r.returncode == 0
    assert "Fedora" in r.stdout
    assert "Trust chain" in r.stdout


def test_show_by_seq_number():
    r = run(
        "show",
        "7",
        "--path",
        str(FIXTURES / "trust-log.jsonl"),
    )
    assert r.returncode == 0
    assert "image_verified" in r.stdout
    assert "shim_mok" in r.stdout


def test_show_by_event_name():
    r = run(
        "show",
        "image_verified",
        "--path",
        str(FIXTURES / "trust-log.jsonl"),
    )
    assert r.returncode == 0
    assert "image_verified" in r.stdout


def test_show_missing_event_returns_exit_code_io():
    r = run(
        "show",
        "this_event_does_not_exist",
        "--path",
        str(FIXTURES / "trust-log.jsonl"),
    )
    assert r.returncode == 2


def test_trust_log_strict_on_clean_fixture_returns_ok():
    r = run(
        "trust-log",
        "-p",
        str(FIXTURES / "trust-log.jsonl"),
        "--strict",
    )
    assert r.returncode == 0


def test_trust_log_nonexistent_path_returns_io_error():
    r = run(
        "trust-log",
        "-p",
        "/nonexistent/path/trust.log",
    )
    assert r.returncode == 2


def test_verify_against_current_checkout_runs_without_crash():
    r = run(
        "verify",
        "--repo",
        str(HERE.parent.parent.parent),
    )
    # Return code may be 0 or 5 depending on branch; we just check that
    # the command finishes and prints the claim list.
    assert r.returncode in (0, 5)
    assert "rust_no_std" in r.stdout
