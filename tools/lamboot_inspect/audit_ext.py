"""Audit-grade inspect features: history, diff, attest.

These build on the trust-log parser + the diagnose rules engine to make the
boot record *auditable over time* (SPEC-LAMBOOT-BOOT-AUDIT-AND-HYGIENE §2):

* ``history`` — list retained past boots with a per-boot verdict.
* ``diff``    — compare two boots and flag regressions (trust downgrade, a new
                preflight issue, a changed kernel sha).
* ``attest``  — surface measured-boot state + export a boot attestation.

History rotation (writing ``/loader/lamboot/history/boot-trust.<ts>.log``) is an
emitter/service concern; these readers tolerate its absence and report "no
retained history" rather than failing.
"""
from __future__ import annotations

import argparse
import json
import pathlib
import re
import sys
from typing import Optional

from . import diagnose as diag
from . import envelope as env_mod
from . import registry as registry_mod
from . import trust_log as tl

HISTORY_REL = "loader/lamboot/history"


def _esp_root(resolve_default) -> "Optional[pathlib.Path]":
    p = resolve_default("trust_log", None)
    # ESP/loader/boot-trust.log -> ESP
    return p.parent.parent if p else None


def _verdict(result: tl.ParseResult) -> str:
    findings = diag.evaluate(diag.Ctx(result, tl.summarize(result)))
    if any(f.severity in (env_mod.SEV_ERROR, env_mod.SEV_CRITICAL) for f in findings):
        return "problems"
    if any(f.severity == env_mod.SEV_WARNING for f in findings):
        return "warnings"
    return "clean"


def _history_logs(resolve_default, explicit: Optional[str]) -> "list[pathlib.Path]":
    base = pathlib.Path(explicit) if explicit else None
    if base is None:
        esp = _esp_root(resolve_default)
        base = (esp / HISTORY_REL) if esp else None
    if base is None or not base.is_dir():
        return []
    return sorted(base.glob("boot-trust.*.log"))


# ── history ───────────────────────────────────────────────────────────────────

def run_history(args, resolve_default, emit, io_error) -> int:
    from .cli import EXIT_OK

    logs = _history_logs(resolve_default, getattr(args, "path", None))
    if getattr(args, "limit", None):
        logs = logs[-args.limit:]
    rows = []
    for p in logs:
        try:
            r = tl.parse(p)
        except (OSError, ValueError):
            continue
        rows.append({"log": p.name, "events": len(r.events), "verdict": _verdict(r)})

    if args.json:
        return emit(args, EXIT_OK, [], {"boots": rows, "count": len(rows)})

    if not rows:
        sys.stdout.write("No retained boot history found "
                         "(rotation writes /loader/lamboot/history/boot-trust.*.log).\n")
        return EXIT_OK
    for row in rows:
        sys.stdout.write(f"  {row['verdict']:9} {row['log']}  ({row['events']} events)\n")
    return EXIT_OK


# ── diff ──────────────────────────────────────────────────────────────────────

def _loaded(result: tl.ParseResult) -> "Optional[tl.TrustEvent]":
    return result.find_loaded_native() or result.find_verified_load()


def run_diff(args, resolve_default, emit, io_error) -> int:
    from .cli import EXIT_OK, EXIT_PARTIAL

    cur = resolve_default("trust_log", None)
    history = _history_logs(resolve_default, None)

    to_path = pathlib.Path(args.to) if getattr(args, "to", None) and args.to != "current" else cur
    if getattr(args, "from_", None):
        from_path = pathlib.Path(args.from_)
    else:
        from_path = history[-1] if history else None

    if from_path is None or not pathlib.Path(from_path).exists():
        return io_error(args, "no baseline boot to diff against (need retained history or --from)")
    if to_path is None or not to_path.exists():
        return io_error(args, f"comparison boot not found ({to_path})")

    a = tl.parse(from_path)
    b = tl.parse(to_path)
    la, lb = _loaded(a), _loaded(b)

    findings: "list[env_mod.Finding]" = []
    # Boot-completion regression: baseline loaded a kernel natively, comparison
    # did not (the strongest diff signal — a previously-good boot now failing).
    if a.find_loaded_native() and not b.find_loaded_native():
        findings.append(env_mod.Finding(
            id="diff.boot_regressed", category="audit", severity=env_mod.SEV_ERROR, status="fail",
            title="Comparison boot did not complete a native image load",
            message="the baseline boot loaded a kernel (image_loaded_native); the comparison did not",
            remediation=env_mod.remediation(
                "The boot regressed since the baseline — inspect the comparison boot's failures.",
                "lamboot-inspect diagnose")))
    # Trust posture downgrade.
    if la and lb and la.verified_via != lb.verified_via:
        sev = env_mod.SEV_WARNING
        if la.verified_via == "shim_mok" and lb.verified_via != "shim_mok":
            sev = env_mod.SEV_ERROR
        findings.append(env_mod.Finding(
            id="diff.trust_changed", category="audit", severity=sev, status="warn",
            title="Boot trust posture changed",
            message=f"verified_via: {la.verified_via or '-'} -> {lb.verified_via or '-'}",
            context={"from": la.verified_via, "to": lb.verified_via},
            remediation=env_mod.remediation(
                "A downgrade (e.g. shim_mok -> degraded_trust_sb_off) means Secure Boot "
                "verification was lost since the baseline boot — investigate.")))
    # Kernel identity changed.
    if la and lb and la.sha256 and lb.sha256 and la.sha256 != lb.sha256:
        findings.append(env_mod.Finding(
            id="diff.kernel_changed", category="audit", severity=env_mod.SEV_INFO, status="info",
            title="Loaded kernel changed",
            message=f"sha256 {la.sha256[:16]}… -> {lb.sha256[:16]}…",
            context={"from": la.sha256, "to": lb.sha256}))
    # New preflight issues.
    na, nb = len(a.by_event("preflight_issue")), len(b.by_event("preflight_issue"))
    if nb > na:
        findings.append(env_mod.Finding(
            id="diff.new_preflight", category="audit", severity=env_mod.SEV_ERROR, status="fail",
            title="New preflight issue(s) appeared",
            message=f"preflight_issue count {na} -> {nb}", context={"from": na, "to": nb}))

    code = EXIT_PARTIAL if any(f.severity in (env_mod.SEV_ERROR, env_mod.SEV_CRITICAL) for f in findings) else EXIT_OK
    if args.json:
        return emit(args, code, findings, {"from": str(from_path), "to": str(to_path)})

    if not findings:
        sys.stdout.write(f"✓ no regressions between {pathlib.Path(from_path).name} and "
                         f"{pathlib.Path(to_path).name}\n")
        return code
    for f in findings:
        sys.stdout.write(f"  [{f.severity}] {f.id}: {f.title} — {f.message}\n")
    return code


# ── attest ────────────────────────────────────────────────────────────────────

_PCR = re.compile(r"pcr=(\d+)")


def run_attest(args, resolve_default, emit, io_error) -> int:
    from .cli import EXIT_OK

    path = resolve_default("trust_log", getattr(args, "path", None))
    if path is None or not path.exists():
        return io_error(args, f"trust log not found (tried {path or 'ESP auto-detect'})")
    r = tl.parse(path)
    loaded = _loaded(r)
    bs = r.by_event("boot_start")
    measured = []
    for name in ("kernel_measured", "cmdline_measured"):
        for e in r.by_event(name):
            m = _PCR.search(e.note or "")
            measured.append({"event": name, "pcr": int(m.group(1)) if m else None})
    tpm_present = bool(measured)

    attestation = {
        "boot": (bs[0].note if bs else ""),
        "loaded_path": loaded.path if loaded else None,
        "loaded_sha256": loaded.sha256 if loaded else None,
        "verified_via": loaded.verified_via if loaded else None,
        "measured_boot": "available" if tpm_present else "unavailable (no TPM)",
        "measurements": measured,
        "source_sha256": r.source_sha256,
    }

    if getattr(args, "out", None):
        try:
            pathlib.Path(args.out).write_text(json.dumps(attestation, indent=2) + "\n")
        except OSError as e:
            return io_error(args, f"cannot write {args.out}: {e}")

    if args.json:
        return emit(args, EXIT_OK, [], {"attestation": attestation})

    sys.stdout.write("Boot attestation\n")
    for k in ("loaded_path", "loaded_sha256", "verified_via", "measured_boot"):
        sys.stdout.write(f"  {k:14} {attestation[k]}\n")
    if measured:
        sys.stdout.write(f"  measurements   {measured}\n")
    if getattr(args, "out", None):
        sys.stdout.write(f"  written        {args.out}\n")
    return EXIT_OK


# ── registry ──────────────────────────────────────────────────────────────────

for _sc in (
    registry_mod.Subcommand(
        name="history", category="Diagnostics",
        summary="List retained past boots with a per-boot trust verdict",
        syntax="lamboot-inspect history [--path DIR] [--limit N] [--json]",
        args=("--path DIR:History directory (default: ESP /loader/lamboot/history)",
              "--limit N:Show at most N past boots"),
        examples=("lamboot-inspect history", "lamboot-inspect history --limit 5 --json"),
        notes="Read-only. Each retained boot gets a clean/warnings/problems verdict via the diagnose rules.",
        see_also=("diff", "diagnose"), doc_url="https://lamboot.dev/tools/inspect/history",
        maturity="draft"),
    registry_mod.Subcommand(
        name="diff", category="Diagnostics",
        summary="Compare two boots and flag regressions (trust, kernel, preflight)",
        syntax="lamboot-inspect diff [--from BOOT] [--to BOOT] [--json]",
        args=("--from BOOT:Baseline boot log (default: previous retained boot)",
              "--to BOOT:Comparison boot log (default: current boot-trust.log)"),
        examples=("lamboot-inspect diff", "lamboot-inspect diff --json"),
        notes="Flags: verified_via downgrade (e.g. shim_mok -> degraded_trust_sb_off), changed kernel sha256, new preflight issues. Read-only.",
        see_also=("history", "diagnose"), doc_url="https://lamboot.dev/tools/inspect/diff",
        maturity="draft"),
    registry_mod.Subcommand(
        name="attest", category="Diagnostics",
        summary="Surface measured-boot state and export a boot attestation",
        syntax="lamboot-inspect attest [--path FILE] [--out FILE] [--json]",
        args=("--out FILE:Write the attestation document (boot identity + sha + PCRs) to FILE",),
        examples=("lamboot-inspect attest", "lamboot-inspect attest --out /tmp/boot.attest.json"),
        notes="Reads kernel_measured/cmdline_measured PCR events; reports measured boot unavailable on TPM-less systems. Read-only.",
        see_also=("diagnose", "verify"), doc_url="https://lamboot.dev/tools/inspect/attest",
        maturity="draft"),
):
    registry_mod.register(_sc)
