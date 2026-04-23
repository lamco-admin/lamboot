"""Tests for lamboot_inspect.boot_log."""
from __future__ import annotations

import datetime as dt
import pathlib
import sys

HERE = pathlib.Path(__file__).resolve()
sys.path.insert(0, str(HERE.parent.parent))

from lamboot_inspect import boot_log as bl  # noqa: E402

FIXTURES = HERE.parent / "fixtures"


def test_parse_fixture_entries():
    parsed = bl.parse(FIXTURES / "boot.log")
    assert parsed.header_version == "0.8.3"
    assert parsed.header_arch == "x86_64"
    assert parsed.header_timestamp == dt.datetime(2026, 4, 22, 0, 14, 0)
    assert len(parsed.entries) >= 9


def test_level_filter_by_level_method():
    parsed = bl.parse(FIXTURES / "boot.log")
    warns = parsed.by_level(bl.Level.WARN)
    assert len(warns) >= 1
    assert all(e.level == bl.Level.WARN for e in warns)


def test_errors_returns_warn_and_error():
    parsed = bl.parse(FIXTURES / "boot.log")
    errs = parsed.errors()
    assert all(e.level in (bl.Level.WARN, bl.Level.ERROR) for e in errs)


def test_missing_timestamp_tolerated(tmp_path):
    f = tmp_path / "boot.log"
    f.write_text(
        "=== LamBoot 0.8.3 (x86_64) boot log ===\n"
        "=== 2026-04-22T00:14:00 ===\n\n"
        "INFO: no-timestamp line\n",
    )
    parsed = bl.parse(f)
    assert any(e.level == bl.Level.INFO and "no-timestamp" in e.message for e in parsed.entries)


def test_continuation_lines_attached_to_previous(tmp_path):
    f = tmp_path / "boot.log"
    f.write_text(
        "[2026-04-22T00:14:01] ERROR: top line\n"
        "    continuation of the error\n"
        "[2026-04-22T00:14:02] INFO: next entry\n",
    )
    parsed = bl.parse(f)
    continuations = [e for e in parsed.entries if e.continuation]
    assert len(continuations) == 1
    assert continuations[0].level == bl.Level.ERROR


def test_phase_extraction_for_drivers():
    parsed = bl.parse(FIXTURES / "boot.log")
    drivers = [p for p in parsed.phases if p.name == "drivers"]
    assert drivers
    # Fixture has "Loading filesystem drivers" at 14:01 and
    # "Loaded 1 filesystem driver(s)" at 14:01 — duration may be 0s.
    phase = drivers[0]
    assert phase.duration_seconds is not None


def test_total_duration_computed():
    parsed = bl.parse(FIXTURES / "boot.log")
    total = parsed.total_duration()
    assert total is not None
    assert total >= 0
