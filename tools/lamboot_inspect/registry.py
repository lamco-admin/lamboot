"""Data-driven subcommand registry — the Python side of the suite help contract.

The bash tools declare their subcommands via `register_subcommand` (13 fields,
0x1F-separated, in `lib/lamboot-toolkit-help.sh`); `scripts/registry-to-man`
sources the tool and renders man/website from that registry. `lamboot-inspect`
is Python, so it can't be sourced — instead it declares the *same* 13 fields
here and emits them in the identical dump format via `--dump-registry`, which
`registry-to-man` consumes for Python tools. One declaration drives argparse
help, the man page, and the website — exactly as for the bash tools.

Field order matches the bash record: name, aliases, category, summary, syntax,
args (|-joined `flag:desc`), examples (||-joined), notes, offline_capable,
requires_root, see_also (comma-joined), doc_url, maturity.
"""
from __future__ import annotations

import dataclasses

FS = "\x1f"  # LAMBOOT_HELP_FS — the bash registry field separator.


@dataclasses.dataclass(frozen=True)
class Subcommand:
    name: str
    category: str
    summary: str
    syntax: str
    args: "tuple[str, ...]" = ()
    examples: "tuple[str, ...]" = ()
    notes: str = ""
    offline_capable: bool = True
    requires_root: bool = False
    see_also: "tuple[str, ...]" = ()
    doc_url: str = ""
    maturity: str = "stable"
    aliases: "tuple[str, ...]" = ()

    def record(self) -> str:
        """Serialize to the 13-field 0x1F record the bash registry produces."""
        return FS.join(
            (
                self.name,
                ",".join(self.aliases),
                self.category,
                self.summary,
                self.syntax,
                "|".join(self.args),
                "||".join(self.examples),
                self.notes,
                "true" if self.offline_capable else "false",
                "true" if self.requires_root else "false",
                ",".join(self.see_also),
                self.doc_url,
                self.maturity,
            )
        )


# Existing subcommands. New analysis subcommands (diagnose/history/diff/attest)
# are appended alongside their implementations so the two never drift.
REGISTRY: "list[Subcommand]" = [
    Subcommand(
        name="trust-log",
        category="Diagnostics",
        summary="Parse and render the LamBoot trust log",
        syntax="lamboot-inspect trust-log [--path FILE] [--format text|json|timeline|stats] [--event NAME] [--errors-only] [--no-sha] [--strict] [--json]",
        args=(
            "--path FILE:Trust log path (default: auto-detect on the ESP)",
            "--format FMT:Render mode — text (default), json, timeline, stats",
            "--event NAME:Show only events of this name",
            "--errors-only:Show only failures and warnings",
            "--no-sha:Omit sha256 digest columns",
            "--strict:Exit UNSAFE (4) if the log has schema/integrity violations",
        ),
        examples=(
            "lamboot-inspect trust-log",
            "lamboot-inspect trust-log --format timeline",
            "lamboot-inspect trust-log --errors-only --json",
        ),
        notes="Read-only. Parses /loader/boot-trust.log (SPEC-NATIVE-TRUST-CHAIN §6). --json emits the shared toolkit envelope; --format json emits the detailed event view.",
        see_also=("summary", "diagnose", "boot-log"),
        doc_url="https://lamboot.dev/tools/inspect/trust-log",
    ),
    Subcommand(
        name="boot-log",
        category="Diagnostics",
        summary="Parse and render the human-readable boot log",
        syntax="lamboot-inspect boot-log [--path FILE] [--level DEBUG|INFO|WARN|ERROR] [--errors-only] [--format text|json] [--json]",
        args=(
            "--path FILE:boot.log path (default: auto-detect on the ESP)",
            "--level LEVEL:Show only entries at this level",
            "--errors-only:Show only warnings and errors",
            "--format FMT:Render mode — text (default) or json",
        ),
        examples=(
            "lamboot-inspect boot-log",
            "lamboot-inspect boot-log --errors-only",
        ),
        notes="Read-only. Parses /EFI/LamBoot/reports/boot.log with optional phase timing.",
        see_also=("trust-log", "summary"),
        doc_url="https://lamboot.dev/tools/inspect/boot-log",
    ),
    Subcommand(
        name="summary",
        category="Diagnostics",
        summary="One-screen overview of the last boot (all artefacts)",
        syntax="lamboot-inspect summary [--trust-path FILE] [--boot-path FILE] [--report-path FILE] [--audit-path FILE] [--json]",
        args=(
            "--trust-path FILE:Override the trust log path",
            "--boot-path FILE:Override the boot.log path",
            "--report-path FILE:Override the boot.json path",
            "--audit-path FILE:Override the audit.log path",
        ),
        examples=(
            "lamboot-inspect summary",
            "lamboot-inspect summary --json",
        ),
        notes="Read-only. Reads the trust log, boot log, boot.json, and audit.log when present.",
        see_also=("diagnose", "trust-log"),
        doc_url="https://lamboot.dev/tools/inspect/summary",
    ),
    Subcommand(
        name="verify",
        category="Diagnostics",
        summary="Verify LamBoot's public claims are backed by code",
        syntax="lamboot-inspect verify [--repo DIR] [--verbose] [--json]",
        args=(
            "--repo DIR:Path to a lamboot-dev checkout (default: current directory)",
            "--verbose:Include SDS spec references",
        ),
        examples=(
            "lamboot-inspect verify --repo ~/lamboot-dev",
            "lamboot-inspect verify --json",
        ),
        notes="Read-only. Checks each website claim against the code path that substantiates it (SDS-4 §8.1).",
        requires_root=False,
        see_also=("summary",),
        doc_url="https://lamboot.dev/tools/inspect/verify",
    ),
    Subcommand(
        name="dump",
        category="Diagnostics",
        summary="Produce a diagnostic bundle (tar.gz) for bug reports",
        syntax="lamboot-inspect dump [--output FILE] [--esp PATH] [--print-manifest]",
        args=(
            "--output FILE:Output bundle path (default: lamboot-inspect-dump-<ts>.tar.gz)",
            "--esp PATH:ESP mount point (override auto-detection)",
            "--print-manifest:Print the sha256 manifest of collected files",
        ),
        examples=(
            "sudo lamboot-inspect dump",
            "sudo lamboot-inspect dump -o /tmp/bundle.tar.gz",
        ),
        notes="Read-only with respect to system state. Collects the ESP diagnostic artefacts into one archive.",
        see_also=("summary",),
        doc_url="https://lamboot.dev/tools/inspect/dump",
    ),
    Subcommand(
        name="show",
        category="Diagnostics",
        summary="Show one trust-log event in detail",
        syntax="lamboot-inspect show EVENT_ID [--path FILE]",
        args=(
            "EVENT_ID:Sequence number, event name, or image path",
            "--path FILE:Override the trust log path",
        ),
        examples=(
            "lamboot-inspect show image_loaded_native",
            "lamboot-inspect show 21",
        ),
        notes="Read-only. Prints every field of the matching event.",
        see_also=("trust-log",),
        doc_url="https://lamboot.dev/tools/inspect/show",
    ),
]


def register(subcommand: Subcommand) -> None:
    """Append a subcommand to the registry (used by analysis modules at import)."""
    if any(s.name == subcommand.name for s in REGISTRY):
        return
    REGISTRY.append(subcommand)


def find(name: str) -> "Subcommand | None":
    for s in REGISTRY:
        if s.name == name or name in s.aliases:
            return s
    return None


def dump(tool_name: str, tool_version: str, toolkit_version: str) -> str:
    """Emit the bash-compatible registry dump consumed by registry-to-man."""
    lines = [
        f"TOOL_NAME={tool_name}",
        f"TOOL_VERSION={tool_version}",
        f"TOOLKIT_VERSION={toolkit_version}",
        "---REGISTRY-BEGIN---",
        "\n".join(s.record() for s in REGISTRY),
        "---REGISTRY-END---",
    ]
    return "\n".join(lines) + "\n"
