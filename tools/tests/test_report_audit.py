"""Tests for lamboot_inspect.report and lamboot_inspect.audit."""
from __future__ import annotations

import pathlib
import sys

HERE = pathlib.Path(__file__).resolve()
sys.path.insert(0, str(HERE.parent.parent))

from lamboot_inspect import audit as audit_mod  # noqa: E402
from lamboot_inspect import report as report_mod  # noqa: E402

FIXTURES = HERE.parent / "fixtures"


def test_boot_json_parses_all_fields():
    r = report_mod.parse(FIXTURES / "boot.json")
    assert r.lamboot_version == "0.8.3"
    assert r.entry_type == "linux_legacy"
    assert r.entry_is_legacy
    assert not r.entry_is_uki
    assert r.os_name == "Fedora Linux"


def test_audit_log_parses_boots():
    parsed = audit_mod.parse(FIXTURES / "audit.log")
    boots = parsed.boots()
    assert len(boots) == 3
    assert boots[-1].timestamp.year == 2026


def test_audit_log_last_entry_has_timestamp():
    parsed = audit_mod.parse(FIXTURES / "audit.log")
    last = parsed.last()
    assert last is not None
    assert last.timestamp is not None
    assert "Fedora" in last.detail
