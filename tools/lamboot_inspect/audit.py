"""Audit-log consumer.

Reads ``\\EFI\\LamBoot\\reports\\audit.log`` — a cumulative line log
recording every boot attempt and explicit error. Each line is of the
form::

    [2026-04-22T00:14:02] Boot: Fedora 43 (bls-fedora-200.fc43)

This parser gives ``lamboot-inspect summary`` a cross-boot history without
needing the full trust log.
"""
from __future__ import annotations

import dataclasses
import datetime as dt
import io
import os
import pathlib
import re
from typing import Iterable, Optional, Union

_LINE_RE = re.compile(r"^\[(?P<ts>[^\]]+)\]\s+(?P<kind>\w+):\s+(?P<rest>.*)$")


@dataclasses.dataclass
class AuditEntry:
    line_number: int
    timestamp: Optional[dt.datetime]
    kind: str
    detail: str
    raw: str


@dataclasses.dataclass
class ParsedAudit:
    entries: list
    source_path: Optional[pathlib.Path] = None

    def boots(self) -> "list[AuditEntry]":
        return [e for e in self.entries if e.kind.lower() == "boot"]

    def errors(self) -> "list[AuditEntry]":
        return [e for e in self.entries if e.kind.lower() == "error"]

    def last(self) -> Optional[AuditEntry]:
        return self.entries[-1] if self.entries else None


def _parse_ts(text: str) -> Optional[dt.datetime]:
    try:
        return dt.datetime.fromisoformat(text)
    except (TypeError, ValueError):
        return None


def parse(
    source: Union[str, bytes, pathlib.Path, io.IOBase, Iterable[str]],
) -> ParsedAudit:
    path: Optional[pathlib.Path] = None
    lines: Iterable[str]
    if isinstance(source, (str, os.PathLike)) and not isinstance(source, bytes):
        path = pathlib.Path(source)
        lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    elif isinstance(source, bytes):
        lines = source.decode("utf-8", errors="replace").splitlines()
    elif hasattr(source, "read"):
        data = source.read()
        text = data.decode("utf-8", errors="replace") if isinstance(data, bytes) else data
        lines = text.splitlines()
    else:
        lines = list(source)

    entries: list = []
    for i, raw in enumerate(lines, start=1):
        line = raw.rstrip("\r")
        if not line.strip():
            continue
        m = _LINE_RE.match(line)
        if not m:
            # Keep the line for rendering but flag it as kind=unknown.
            entries.append(
                AuditEntry(
                    line_number=i,
                    timestamp=None,
                    kind="unknown",
                    detail=line.strip(),
                    raw=line,
                )
            )
            continue
        entries.append(
            AuditEntry(
                line_number=i,
                timestamp=_parse_ts(m.group("ts")),
                kind=m.group("kind"),
                detail=m.group("rest"),
                raw=line,
            )
        )
    return ParsedAudit(entries=entries, source_path=path)
