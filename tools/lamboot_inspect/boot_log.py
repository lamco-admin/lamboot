"""Boot-log parser.

The LamBoot boot log (``\\EFI\\LamBoot\\reports\\boot.log``) is a
human-readable timestamped trace written by ``bootlog.rs``. This module
parses that free-form text into structured records so ``lamboot-inspect``
can show phase timing, filter by level, and highlight errors.

Example input line:

    [2026-04-22T00:14:02] INFO: Mounting ESP filesystem...

The parser tolerates:
* Missing timestamps (pre-RTC boots, virtualized hosts with no clock)
* Lines without a level prefix (legacy emitters, third-party drivers)
* Multi-line messages (continuation lines are attached to the previous
  record rather than dropped)
* CRLF line endings
"""
from __future__ import annotations

import dataclasses
import datetime as dt
import enum
import io
import os
import pathlib
import re
from typing import Iterable, Iterator, Optional, Union


class Level(str, enum.Enum):
    DEBUG = "DEBUG"
    INFO = "INFO"
    WARN = "WARN"
    ERROR = "ERROR"
    UNKNOWN = "UNKNOWN"


_LINE_RE = re.compile(
    r"^\[(?P<ts>[^\]]+)\]\s+(?P<level>[A-Z]+):\s+(?P<msg>.*)$"
)
# Some lines omit the timestamp entirely (unusual but seen on OVMF
# with no RTC). Tolerant fallback.
_NO_TS_RE = re.compile(r"^(?P<level>[A-Z]+):\s+(?P<msg>.*)$")


@dataclasses.dataclass
class BootLogEntry:
    line_number: int
    timestamp: Optional[dt.datetime]
    level: Level
    message: str
    # Whether this entry is the continuation of the previous line
    # (multi-line message). Continuations re-use the previous
    # timestamp/level.
    continuation: bool = False
    raw: str = ""


@dataclasses.dataclass
class Phase:
    """A named boot-phase span between two INFO markers.

    Heuristic phase extraction — LamBoot does not currently emit
    structured phase markers. The parser looks for well-known strings
    in the boot log and reports durations. SDS-4 v1.1 may add explicit
    phase markers; this parser is forward-compatible.
    """

    name: str
    start: Optional[dt.datetime]
    end: Optional[dt.datetime]
    start_line: int
    end_line: int

    @property
    def duration_seconds(self) -> Optional[float]:
        if self.start is None or self.end is None:
            return None
        return (self.end - self.start).total_seconds()


@dataclasses.dataclass
class ParsedBootLog:
    entries: list
    phases: list
    header_version: Optional[str] = None
    header_arch: Optional[str] = None
    header_timestamp: Optional[dt.datetime] = None
    source_path: Optional[pathlib.Path] = None

    def by_level(self, level: Union[Level, str]) -> "list[BootLogEntry]":
        key = level.value if isinstance(level, Level) else level
        return [e for e in self.entries if e.level.value == key]

    def errors(self) -> "list[BootLogEntry]":
        return [
            e
            for e in self.entries
            if e.level in (Level.ERROR, Level.WARN)
        ]

    def total_duration(self) -> Optional[float]:
        timed = [e for e in self.entries if e.timestamp is not None]
        if len(timed) < 2:
            return None
        return (timed[-1].timestamp - timed[0].timestamp).total_seconds()


_HEADER_RE = re.compile(
    r"^=== LamBoot (?P<version>\S+) \((?P<arch>[^)]+)\) boot log ===$"
)
_HEADER_TS_RE = re.compile(r"^=== (?P<ts>[^=]+?) ===$")

# Known phase markers — keep this list small and precise to avoid
# false-positive duration reports.
_PHASE_MARKERS = [
    ("health", "health counter", "Crash counter"),
    ("drivers", "Loading filesystem drivers", "Loaded"),
    ("discovery", "Discovering boot entries", "Found "),
    ("init-total", "Boot init:", "Boot init:"),
]


def _parse_timestamp(text: str) -> Optional[dt.datetime]:
    # LamBoot emits ISO-8601 without timezone ("2026-04-22T00:14:02").
    # We keep this parser permissive so third-party logs with other
    # ISO-adjacent formats still surface a timestamp when they can.
    try:
        return dt.datetime.fromisoformat(text)
    except (TypeError, ValueError):
        pass
    for fmt in ("%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S", "%H:%M:%S"):
        try:
            return dt.datetime.strptime(text, fmt)
        except (TypeError, ValueError):
            continue
    return None


def _parse_level(text: str) -> Level:
    upper = text.upper()
    if upper in ("DEBUG", "INFO", "WARN", "WARNING", "ERROR", "ERR"):
        if upper in ("WARNING",):
            return Level.WARN
        if upper in ("ERR",):
            return Level.ERROR
        return Level(upper)
    return Level.UNKNOWN


def parse(
    source: Union[str, bytes, pathlib.Path, io.IOBase, Iterable[str]],
) -> ParsedBootLog:
    """Parse a boot.log into a :class:`ParsedBootLog`.

    The source types accepted mirror those of
    :func:`lamboot_inspect.trust_log.parse`.
    """
    path: Optional[pathlib.Path] = None
    lines: Iterable[str]

    if isinstance(source, (str, os.PathLike)) and not isinstance(source, bytes):
        path = pathlib.Path(source)
        lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    elif isinstance(source, bytes):
        lines = source.decode("utf-8", errors="replace").splitlines()
    elif isinstance(source, io.IOBase) or hasattr(source, "read"):
        text = source.read()
        if isinstance(text, bytes):
            text = text.decode("utf-8", errors="replace")
        lines = text.splitlines()
    else:
        lines = list(source)

    entries: list = []
    header_version: Optional[str] = None
    header_arch: Optional[str] = None
    header_ts: Optional[dt.datetime] = None

    last_entry: Optional[BootLogEntry] = None
    for i, raw in enumerate(lines, start=1):
        line = raw.rstrip("\r")
        if not line.strip():
            last_entry = None
            continue
        h = _HEADER_RE.match(line)
        if h:
            header_version = h.group("version")
            header_arch = h.group("arch")
            last_entry = None
            continue
        ht = _HEADER_TS_RE.match(line)
        if ht and header_version is not None and header_ts is None:
            header_ts = _parse_timestamp(ht.group("ts").strip())
            last_entry = None
            continue

        m = _LINE_RE.match(line)
        if m:
            entry = BootLogEntry(
                line_number=i,
                timestamp=_parse_timestamp(m.group("ts")),
                level=_parse_level(m.group("level")),
                message=m.group("msg"),
                raw=line,
            )
            entries.append(entry)
            last_entry = entry
            continue

        m2 = _NO_TS_RE.match(line)
        if m2:
            entry = BootLogEntry(
                line_number=i,
                timestamp=None,
                level=_parse_level(m2.group("level")),
                message=m2.group("msg"),
                raw=line,
            )
            entries.append(entry)
            last_entry = entry
            continue

        # Continuation of the previous entry.
        if last_entry is not None:
            continuation = BootLogEntry(
                line_number=i,
                timestamp=last_entry.timestamp,
                level=last_entry.level,
                message=line.strip(),
                continuation=True,
                raw=line,
            )
            entries.append(continuation)

    phases = _extract_phases(entries)
    return ParsedBootLog(
        entries=entries,
        phases=phases,
        header_version=header_version,
        header_arch=header_arch,
        header_timestamp=header_ts,
        source_path=path,
    )


def _extract_phases(entries: "list[BootLogEntry]") -> "list[Phase]":
    """Heuristic phase extraction.

    For each registered marker tuple (name, start_substring,
    end_substring), find the first INFO entry whose message contains the
    start substring, then the next matching end substring. Emit a
    :class:`Phase` with durations derived from timestamps when present.

    This is best-effort — if the log is truncated or markers are
    missing, the affected phase is omitted.
    """
    phases: list = []
    for name, start_sub, end_sub in _PHASE_MARKERS:
        start_idx = next(
            (i for i, e in enumerate(entries) if start_sub in e.message),
            None,
        )
        if start_idx is None:
            continue
        # Find matching end from the start forward. Allow the same
        # substring to be both start and end (e.g. "Boot init:") —
        # in that case start_idx is the match.
        end_idx = next(
            (
                i
                for i in range(start_idx + (0 if start_sub == end_sub else 1), len(entries))
                if end_sub in entries[i].message
            ),
            None,
        )
        if end_idx is None:
            end_idx = start_idx
        start_entry = entries[start_idx]
        end_entry = entries[end_idx]
        phases.append(
            Phase(
                name=name,
                start=start_entry.timestamp,
                end=end_entry.timestamp,
                start_line=start_entry.line_number,
                end_line=end_entry.line_number,
            )
        )
    return phases
