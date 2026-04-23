"""Tests for lamboot_inspect.trust_log.

Covers:
  * Happy-path parsing of the v2 event schema
  * Defensive parsing: unknown fields, missing fields, malformed JSON
  * Validation: SHA-256 format, stable verified_via tokens,
    image_verified invariants
  * Statistics aggregation
"""
from __future__ import annotations

import json
import pathlib
import sys

# Make the lamboot_inspect package importable without an install.
HERE = pathlib.Path(__file__).resolve()
sys.path.insert(0, str(HERE.parent.parent))

from lamboot_inspect import trust_log as tl  # noqa: E402


FIXTURES = HERE.parent / "fixtures"


def test_parse_happy_path_returns_all_events():
    result = tl.parse(FIXTURES / "trust-log.jsonl")
    assert len(result.events) == 13
    assert not result.parse_errors
    assert not result.schema_violations, [v.issues for v in result.schema_violations]
    assert result.events[0].event == "boot_start"
    assert result.events[-1].event == "boot_attempt"


def test_find_verified_load_returns_last_image_verified():
    result = tl.parse(FIXTURES / "trust-log.jsonl")
    verified = result.find_verified_load()
    assert verified is not None
    assert verified.event == "image_verified"
    assert verified.verified_via == "shim_mok"
    assert verified.sha256 == (
        "3df266502d2443ed68a609a0b7e3febf0d37fca52deb704de15637990e8dea82"
    )


def test_sha256_match_across_verify_and_load():
    result = tl.parse(FIXTURES / "trust-log.jsonl")
    stats = tl.summarize(result)
    assert stats.sha256_verify_vs_load_match is True


def test_failure_event_marked_as_failure():
    result = tl.parse(FIXTURES / "trust-log-failed.jsonl")
    failures = result.failures()
    assert {e.event for e in failures} >= {"image_load_failed", "kernel_load_failed"}


def test_degraded_paths_classified_as_warn():
    result = tl.parse(FIXTURES / "trust-log-failed.jsonl")
    warn_events = [e for e in result.events if e.classification() == "warn"]
    # driver_loaded with security_override is a warn path.
    assert any(e.verified_via == "security_override" for e in warn_events)


def test_parse_tolerates_malformed_json_line(tmp_path):
    f = tmp_path / "bad.log"
    f.write_text(
        '{"seq":0,"event":"boot_start"}\n'
        "not valid json at all\n"
        '{"seq":2,"event":"image_verified","sha256":"abc","verified_via":"shim_mok"}\n',
    )
    result = tl.parse(f)
    # Good lines parsed; bad line flagged.
    assert len(result.events) == 2
    assert len(result.parse_errors) == 1
    err = result.parse_errors[0]
    assert err.line_number == 2
    assert "json decode" in err.reason


def test_unknown_fields_preserved_in_extra():
    line = json.dumps(
        {
            "seq": 42,
            "event": "boot_start",
            "future_field": "hello",
            "another": [1, 2, 3],
        }
    )
    result = tl.parse((line + "\n").encode("utf-8"))
    assert len(result.events) == 1
    assert result.events[0].extra == {"future_field": "hello", "another": [1, 2, 3]}


def test_schema_violation_for_bad_sha256():
    line = json.dumps(
        {
            "seq": 1,
            "event": "image_verified",
            "sha256": "not-hex-at-all",
            "verified_via": "shim_mok",
        }
    )
    result = tl.parse((line + "\n").encode("utf-8"))
    assert result.schema_violations
    issues = result.schema_violations[0].issues
    assert any("64-char hex digest" in issue for issue in issues)


def test_schema_violation_for_unknown_verified_via():
    line = json.dumps(
        {
            "seq": 1,
            "event": "image_verified",
            "sha256": "a" * 64,
            "verified_via": "invented_token",
        }
    )
    result = tl.parse((line + "\n").encode("utf-8"))
    assert any(
        "stable vocabulary" in issue
        for v in result.schema_violations
        for issue in v.issues
    )


def test_schema_violation_for_image_verified_without_sha():
    line = json.dumps(
        {"seq": 1, "event": "image_verified", "verified_via": "shim_mok"}
    )
    result = tl.parse((line + "\n").encode("utf-8"))
    assert any(
        "SDS-4 §6.4" in issue
        for v in result.schema_violations
        for issue in v.issues
    )


def test_statistics_include_all_fields():
    result = tl.parse(FIXTURES / "trust-log.jsonl")
    stats = tl.summarize(result)
    assert stats.total_events == 13
    assert stats.boot_attempts == 1
    assert stats.failures == 0
    assert stats.latest_verified_path == "/boot/vmlinuz-6.12.0-fedora"
    assert "image_verified" in stats.by_event
    assert stats.by_verified_via.get("shim_mok", 0) >= 2


def test_events_sorted_by_seq_on_out_of_order_source(tmp_path):
    f = tmp_path / "ordering.log"
    lines = [
        json.dumps({"seq": 3, "event": "boot_attempt"}),
        json.dumps({"seq": 1, "event": "boot_start"}),
        json.dumps({"seq": 2, "event": "volume_mounted"}),
    ]
    f.write_text("\n".join(lines) + "\n")
    result = tl.parse(f)
    assert [e.seq for e in result.events] == [1, 2, 3]


def test_parse_from_bytes_also_works():
    result = tl.parse(FIXTURES.joinpath("trust-log.jsonl").read_bytes())
    assert len(result.events) == 13
    assert result.source_sha256  # bytes source also computes sha


def test_classification_on_all_known_events():
    result = tl.parse(FIXTURES / "trust-log.jsonl")
    for event in result.events:
        assert event.classification() in {"ok", "fail", "warn", "info"}


def test_verified_via_enum_values_match_stable_tokens():
    # Every enum value must appear in VERIFIED_VIA_TOKENS.
    for member in tl.VerifiedVia:
        assert member.value in tl.VERIFIED_VIA_TOKENS
