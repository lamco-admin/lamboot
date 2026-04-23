"""Trust-log parser.

Implements the schema v2 defined in ``docs/specs/SPEC-NATIVE-TRUST-CHAIN.md``
§6.2 and the stable ``verified_via`` vocabulary from §6.3.

Defensive-parsing rules:

* Every line is an independent JSON document. A malformed line does not
  break parsing — it is captured as :class:`ParseError` with the line
  number and raw content for diagnostics.
* Unknown fields on a record are preserved in :attr:`TrustEvent.extra`.
  Forward-compatible: a future schema v3 addition is visible to humans
  running an older ``lamboot-inspect`` without crashing it.
* Unknown ``verified_via`` tokens are retained literally — useful for
  spotting typos or schema drift during development.
* Events missing required fields are surfaced as
  :class:`SchemaViolation` with the offending field list. The caller
  decides whether to tolerate or reject them; by default the renderer
  shows them with a ⚠ marker.

The parser is intentionally "lenient in, strict out": it accepts
imperfect logs and produces a typed stream plus a diagnostics channel.
"""
from __future__ import annotations

import dataclasses
import enum
import hashlib
import io
import json
import os
import pathlib
import re
from typing import Iterable, Iterator, Optional, Union

# ---------------------------------------------------------------------------
# Event identifiers — authoritative per SPEC-NATIVE-TRUST-CHAIN §6.1
# ---------------------------------------------------------------------------


class Event(str, enum.Enum):
    """Known trust-log event identifiers.

    Stored as a ``str`` subclass so JSON ``event`` fields compare
    directly. Unknown events are retained as bare strings rather than
    enum members — see :func:`classify_event`.
    """

    BOOT_START = "boot_start"
    VOLUME_MOUNTED = "volume_mounted"
    SHIMLOCK_ACQUIRED = "shimlock_acquired"
    SHIMLOCK_ABSENT = "shimlock_absent"
    SHIM_RETAIN_REQUESTED = "shim_retain_requested"
    POLICY_LOADED = "policy_loaded"
    POLICY_INVALID = "policy_invalid"
    ENTRIES_DISCOVERED = "entries_discovered"
    ENTRY_SELECTED = "entry_selected"
    KERNEL_BYTES_READ = "kernel_bytes_read"
    IMAGE_VERIFIED = "image_verified"
    KERNEL_MEASURED = "kernel_measured"
    CMDLINE_MEASURED = "cmdline_measured"
    IMAGE_LOADED_NATIVE = "image_loaded_native"
    IMAGE_LOAD_FAILED = "image_load_failed"
    INITRD_REGISTERED = "initrd_registered"
    DRIVER_LOADED = "driver_loaded"
    DRIVER_REJECTED = "driver_rejected"
    BOOT_ATTEMPT = "boot_attempt"
    KERNEL_LOAD_FAILED = "kernel_load_failed"
    TPM_ABSENT = "tpm_absent"


# Permitted tokens in the `verified_via` field per §6.3.
# Order: most-preferred to least-preferred degradation.
VERIFIED_VIA_TOKENS = (
    "shim_mok",
    "shim_vendor",
    "firmware_db_fallback",
    "shim_sbat_rejected",
    "shim_not_enrolled",
    "shim_absent_after_driver_load",
    "firmware_db_rejected",
    "degraded_trust_sb_off",
    "security_override",
    "rejected",
    "sb_disabled",
)


class VerifiedVia(str, enum.Enum):
    SHIM_MOK = "shim_mok"
    SHIM_VENDOR = "shim_vendor"
    FIRMWARE_DB_FALLBACK = "firmware_db_fallback"
    SHIM_SBAT_REJECTED = "shim_sbat_rejected"
    SHIM_NOT_ENROLLED = "shim_not_enrolled"
    SHIM_ABSENT_AFTER_DRIVER_LOAD = "shim_absent_after_driver_load"
    FIRMWARE_DB_REJECTED = "firmware_db_rejected"
    DEGRADED_TRUST_SB_OFF = "degraded_trust_sb_off"
    SECURITY_OVERRIDE = "security_override"
    REJECTED = "rejected"
    SB_DISABLED = "sb_disabled"


# ---------------------------------------------------------------------------
# Dataclasses
# ---------------------------------------------------------------------------


@dataclasses.dataclass
class TrustEvent:
    """A single parsed trust event.

    Fields correspond 1:1 to the schema-v2 JSON keys. Fields not present
    in the source JSON default to empty strings / zero so the downstream
    renderer never has to special-case missing data.
    """

    seq: int = 0
    event: str = ""
    path: str = ""
    size: int = 0
    sha256: str = ""
    verified_via: str = ""
    verifier_tag: str = ""
    status: str = ""
    note: str = ""
    # Anything else present in the source JSON line.
    extra: dict = dataclasses.field(default_factory=dict)
    # Populated by the parser, not by the log source.
    source_line: int = 0

    def is_success(self) -> bool:
        """True if the event represents a successful outcome.

        Success here means the boot made forward progress at this step.
        Anything explicitly marked as a failure — image_load_failed,
        kernel_load_failed, shim_sbat_rejected, firmware_db_rejected —
        is NOT success.
        """
        if self.event in (
            Event.IMAGE_LOAD_FAILED,
            Event.KERNEL_LOAD_FAILED,
            Event.POLICY_INVALID,
            Event.DRIVER_REJECTED,
        ):
            return False
        if self.status and self.status not in ("SUCCESS", "Success", ""):
            return False
        if self.verified_via in (
            "shim_sbat_rejected",
            "shim_not_enrolled",
            "shim_absent_after_driver_load",
            "firmware_db_rejected",
            "rejected",
        ):
            return False
        return True

    def is_failure(self) -> bool:
        return not self.is_success()

    def classification(self) -> str:
        """Short label: ``ok`` / ``fail`` / ``warn`` / ``info``.

        - ``ok``   — success outcome on a load-bearing step
          (image_verified / image_loaded_native / boot_attempt)
        - ``fail`` — explicit failure event
        - ``warn`` — degraded-trust paths (SB off, security_override,
          firmware_db_fallback)
        - ``info`` — everything else
        """
        if self.is_failure():
            return "fail"
        if self.verified_via in (
            "degraded_trust_sb_off",
            "security_override",
            "firmware_db_fallback",
            "sb_disabled",
        ):
            return "warn"
        if self.event in (
            Event.IMAGE_VERIFIED,
            Event.IMAGE_LOADED_NATIVE,
            Event.BOOT_ATTEMPT,
            Event.KERNEL_MEASURED,
        ):
            return "ok"
        return "info"


@dataclasses.dataclass
class ParseError:
    """A line that could not be parsed as JSON at all."""

    line_number: int
    raw: str
    reason: str


@dataclasses.dataclass
class SchemaViolation:
    """A parsed record that is missing fields or uses illegal values."""

    line_number: int
    event: TrustEvent
    issues: list


@dataclasses.dataclass
class ParseResult:
    """Outcome of parsing one log."""

    events: list
    parse_errors: list
    schema_violations: list
    source_path: Optional[pathlib.Path] = None
    source_sha256: Optional[str] = None

    def by_event(self, event: Union[Event, str]) -> "list[TrustEvent]":
        key = event.value if isinstance(event, Event) else event
        return [e for e in self.events if e.event == key]

    def find_verified_load(self) -> Optional[TrustEvent]:
        """Return the last image_verified event, if any."""
        verified = self.by_event(Event.IMAGE_VERIFIED)
        return verified[-1] if verified else None

    def find_loaded_native(self) -> Optional[TrustEvent]:
        native = self.by_event(Event.IMAGE_LOADED_NATIVE)
        return native[-1] if native else None

    def boot_attempt(self) -> Optional[TrustEvent]:
        attempts = self.by_event(Event.BOOT_ATTEMPT)
        return attempts[-1] if attempts else None

    def failures(self) -> "list[TrustEvent]":
        return [e for e in self.events if e.is_failure()]

    def warnings(self) -> "list[TrustEvent]":
        return [e for e in self.events if e.classification() == "warn"]


# ---------------------------------------------------------------------------
# Parser
# ---------------------------------------------------------------------------

_REQUIRED_FIELDS = ("event",)
_KNOWN_FIELDS = frozenset(
    {
        "seq",
        "event",
        "path",
        "size",
        "sha256",
        "verified_via",
        "verifier_tag",
        "status",
        "note",
    }
)


def _coerce_int(value) -> int:
    if isinstance(value, bool):
        # bool is a subclass of int — reject explicitly.
        return 0
    if isinstance(value, int):
        return value
    if isinstance(value, str) and value.isdigit():
        return int(value)
    return 0


def _coerce_str(value) -> str:
    if value is None:
        return ""
    if isinstance(value, str):
        return value
    return str(value)


def parse_line(raw_line: str, line_number: int) -> Union[TrustEvent, ParseError]:
    """Parse a single JSON-lines record.

    Returns a :class:`TrustEvent` on success or a :class:`ParseError`
    with the raw line and a description of what failed.
    """
    stripped = raw_line.strip()
    if not stripped:
        return ParseError(line_number, raw_line, "empty line")
    try:
        obj = json.loads(stripped)
    except json.JSONDecodeError as e:
        return ParseError(line_number, raw_line, f"json decode: {e.msg} at col {e.colno}")
    if not isinstance(obj, dict):
        return ParseError(line_number, raw_line, f"expected object, got {type(obj).__name__}")

    event = TrustEvent(
        seq=_coerce_int(obj.get("seq", 0)),
        event=_coerce_str(obj.get("event", "")),
        path=_coerce_str(obj.get("path", "")),
        size=_coerce_int(obj.get("size", 0)),
        sha256=_coerce_str(obj.get("sha256", "")),
        verified_via=_coerce_str(obj.get("verified_via", "")),
        verifier_tag=_coerce_str(obj.get("verifier_tag", "")),
        status=_coerce_str(obj.get("status", "")),
        note=_coerce_str(obj.get("note", "")),
        extra={k: v for k, v in obj.items() if k not in _KNOWN_FIELDS},
        source_line=line_number,
    )
    return event


def classify_event(event_name: str) -> Optional[Event]:
    """Return the :class:`Event` variant matching ``event_name`` or None."""
    try:
        return Event(event_name)
    except ValueError:
        return None


def validate(event: TrustEvent) -> list:
    """Return a list of schema-violation descriptions for ``event``.

    An empty list means the record is schema-clean. Violations are
    strings intended for operator-facing display, not machine-readable.
    """
    issues: list = []
    if not event.event:
        issues.append("missing required field: event")
    if event.sha256 and not re.fullmatch(r"[0-9a-fA-F]{64}", event.sha256):
        issues.append("sha256 field is not a 64-char hex digest")
    if (
        event.verified_via
        and event.verified_via not in VERIFIED_VIA_TOKENS
        and not event.verified_via.startswith("_")
    ):
        issues.append(
            f"verified_via token '{event.verified_via}' is not in the stable vocabulary"
        )
    # SDS-4 §6.4: image_verified SHOULD carry a sha256.
    if event.event == Event.IMAGE_VERIFIED and not event.sha256:
        issues.append("image_verified event is missing sha256 (SDS-4 §6.4 invariant)")
    if event.event == Event.IMAGE_VERIFIED and not event.verified_via:
        issues.append("image_verified event is missing verified_via (SDS-4 §6.2)")
    return issues


def parse(
    source: Union[str, bytes, pathlib.Path, io.IOBase, Iterable[str]],
    *,
    compute_sha256: bool = True,
) -> ParseResult:
    """Parse a trust log from a path, bytes, file-like object, or iterable of lines.

    Args:
        source: One of:
            * :class:`pathlib.Path` or ``str`` path to a log file on disk
            * ``bytes`` (the raw log contents)
            * File-like object returning text
            * An iterable of strings (one per line)
        compute_sha256: When ``source`` is a path or bytes, compute a
            SHA-256 of the raw input so downstream consumers can tag
            exported reports with a content hash. Defaults to True.

    Returns:
        :class:`ParseResult` with events, parse errors, schema
        violations, and optional source metadata.
    """
    path: Optional[pathlib.Path] = None
    source_sha = None
    lines: Iterable[str]

    if isinstance(source, (str, os.PathLike)) and not isinstance(source, bytes):
        path = pathlib.Path(source)
        raw_bytes = path.read_bytes()
        if compute_sha256:
            source_sha = hashlib.sha256(raw_bytes).hexdigest()
        lines = raw_bytes.decode("utf-8", errors="replace").splitlines()
    elif isinstance(source, bytes):
        if compute_sha256:
            source_sha = hashlib.sha256(source).hexdigest()
        lines = source.decode("utf-8", errors="replace").splitlines()
    elif isinstance(source, io.IOBase) or hasattr(source, "read"):
        text = source.read()
        if isinstance(text, bytes):
            if compute_sha256:
                source_sha = hashlib.sha256(text).hexdigest()
            text = text.decode("utf-8", errors="replace")
        lines = text.splitlines()
    else:
        lines = list(source)

    events: list = []
    parse_errors: list = []
    schema_violations: list = []

    for i, line in enumerate(lines, start=1):
        if not line.strip():
            continue
        result = parse_line(line, i)
        if isinstance(result, ParseError):
            parse_errors.append(result)
            continue
        events.append(result)
        issues = validate(result)
        if issues:
            schema_violations.append(SchemaViolation(i, result, issues))

    # Sort by seq when available — boots may emit events out of order if
    # the log is merged across multiple flushes in an unusual sequence.
    # Stable sort keeps source order for records sharing seq.
    if events and any(e.seq for e in events):
        events.sort(key=lambda e: (e.seq, e.source_line))

    return ParseResult(
        events=events,
        parse_errors=parse_errors,
        schema_violations=schema_violations,
        source_path=path,
        source_sha256=source_sha,
    )


# ---------------------------------------------------------------------------
# Statistics
# ---------------------------------------------------------------------------


@dataclasses.dataclass
class Statistics:
    total_events: int
    by_event: dict
    by_verified_via: dict
    failures: int
    warnings: int
    latest_verified_path: Optional[str]
    latest_verified_sha256: Optional[str]
    sha256_verify_vs_load_match: Optional[bool]
    boot_attempts: int


def summarize(result: ParseResult) -> Statistics:
    """Aggregate a ``ParseResult`` into summary statistics."""
    by_event: dict = {}
    by_verified_via: dict = {}
    failures = 0
    warnings = 0
    for e in result.events:
        by_event[e.event] = by_event.get(e.event, 0) + 1
        if e.verified_via:
            by_verified_via[e.verified_via] = by_verified_via.get(e.verified_via, 0) + 1
        cls = e.classification()
        if cls == "fail":
            failures += 1
        elif cls == "warn":
            warnings += 1

    verified = result.find_verified_load()
    loaded = result.find_loaded_native()
    invariant: Optional[bool] = None
    if verified and loaded and verified.sha256 and loaded.sha256:
        invariant = verified.sha256 == loaded.sha256

    return Statistics(
        total_events=len(result.events),
        by_event=by_event,
        by_verified_via=by_verified_via,
        failures=failures,
        warnings=warnings,
        latest_verified_path=verified.path if verified else None,
        latest_verified_sha256=verified.sha256 if verified else None,
        sha256_verify_vs_load_match=invariant,
        boot_attempts=by_event.get(Event.BOOT_ATTEMPT, 0),
    )
