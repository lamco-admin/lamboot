"""`lamboot-inspect diagnose` — interpret the last boot.

Where `trust-log`/`summary` *render* the artefacts, `diagnose` *interprets*
them: a rules catalogue maps event signatures to operator-facing findings, each
with a recommendation. It is the boot-event analogue of the lamboot-migrate
troubleshooting catalogue (SPEC-LAMBOOT-BOOT-AUDIT-AND-HYGIENE §1.3).

Rules are data: each is a small function over a :class:`Ctx` returning zero or
more :class:`envelope.Finding`. Adding a failure class is adding a rule — the
renderer never changes. The catalogue is keyed to the *real* trust-log
vocabulary (the live VM-129 log is the reference sample), and matches event
strings directly so newer emitter events (e.g. ``preflight_issue``,
``kernel_measurement_skipped``) are handled even though the schema-v2 enum in
:mod:`trust_log` predates them.
"""
from __future__ import annotations

import argparse
import dataclasses
import json
import re
import sys
from typing import Callable, Optional

from . import envelope as env_mod
from . import registry as registry_mod
from . import trust_log as tl

CATEGORY = "boot"

# Severity rank for the --severity floor and exit-code reduction.
_RANK = {env_mod.SEV_INFO: 0, env_mod.SEV_WARNING: 1, env_mod.SEV_ERROR: 2, env_mod.SEV_CRITICAL: 3}


@dataclasses.dataclass
class Ctx:
    """Everything a rule may inspect about the last boot."""

    result: tl.ParseResult
    stats: tl.Statistics
    boot_json: Optional[dict] = None

    def events(self, name: str) -> "list[tl.TrustEvent]":
        return self.result.by_event(name)

    def first(self, name: str) -> Optional[tl.TrustEvent]:
        evs = self.events(name)
        return evs[0] if evs else None


def _rec(summary: str, command: str = "", doc_url: str = "") -> dict:
    return env_mod.remediation(summary, command, doc_url)


def _f(
    rule_id: str,
    severity: str,
    status: str,
    title: str,
    message: str,
    *,
    context: Optional[dict] = None,
    remediation: Optional[dict] = None,
) -> env_mod.Finding:
    return env_mod.Finding(
        id=rule_id,
        category=CATEGORY,
        severity=severity,
        status=status,
        title=title,
        message=message,
        context=context or {},
        remediation=remediation,
    )


# ---------------------------------------------------------------------------
# Rules — each returns a list of findings (possibly empty)
# ---------------------------------------------------------------------------

_DEGRADED_REC = _rec(
    "Secure Boot is off, so the kernel was not cryptographically verified. "
    "Enrol a MOK (Config 3) or use pre-enrolled OVMF VARS (Config 4) for real trust.",
    doc_url="https://lamboot.dev/secure-boot",
)


def rule_trust_token(ctx: Ctx) -> "list[env_mod.Finding]":
    verified = ctx.result.find_verified_load()
    if not verified:
        return []
    via = verified.verified_via
    base = {"seq": verified.seq, "verified_via": via, "sha256": verified.sha256}
    if via == "degraded_trust_sb_off":
        return [_f("trust.sb_off", env_mod.SEV_WARNING, "warn",
                   "Kernel was not cryptographically verified (Secure Boot off)",
                   "image_verified recorded degraded_trust_sb_off — firmware trusted the image, "
                   "no shim/MOK verification occurred.", context=base, remediation=_DEGRADED_REC)]
    if via in ("security_override", "sb_disabled"):
        return [_f("trust.sb_off", env_mod.SEV_WARNING, "warn",
                   "Kernel trust degraded (Secure Boot disabled / overridden)",
                   f"verified_via={via}", context=base, remediation=_DEGRADED_REC)]
    if via == "firmware_db_fallback":
        return [_f("trust.sb_direct", env_mod.SEV_WARNING, "warn",
                   "Image trusted directly via firmware db (not shim/MOK)",
                   "verified_via=firmware_db_fallback — confirm the signing chain is intended.",
                   context=base)]
    if via in ("shim_sbat_rejected", "shim_not_enrolled", "shim_absent_after_driver_load",
               "firmware_db_rejected", "rejected"):
        return [_f("trust.shim_rejected", env_mod.SEV_ERROR, "fail",
                   "Image verification was REJECTED",
                   f"verified_via={via} — the image was refused by the trust chain.",
                   context=base,
                   remediation=_rec("Check MOK enrolment and the kernel signature.",
                                    "sudo lamboot-signing-keys status"))]
    return []


def rule_preflight(ctx: Ctx) -> "list[env_mod.Finding]":
    out = []
    for e in ctx.events("preflight_issue"):
        out.append(_f("boot.preflight_issue", env_mod.SEV_ERROR, "fail",
                      "A boot entry failed preflight (red-✗)",
                      e.note or "entry rejected before load",
                      context={"seq": e.seq, "path": e.path, "note": e.note},
                      remediation=_rec(
                          "Map the message to a fix: symlink unreadable -> upgrade the FS "
                          "backend; driver missing -> deploy the <fs>_x64.efi driver; initrd "
                          "not found -> regenerate the initramfs.",
                          "sudo lamboot-inspect trust-log --errors-only")))
    return out


def rule_crashloop(ctx: Ctx) -> "list[env_mod.Finding]":
    bs = ctx.first("boot_start")
    if not bs:
        return []
    m = re.search(r"crash_counter=(\d+)", bs.note or "")
    if not m or int(m.group(1)) == 0:
        return []
    n = int(m.group(1))
    return [_f("boot.crashloop", env_mod.SEV_WARNING, "warn",
               f"Previous boot(s) failed — crash counter is {n}",
               "Crash-loop recovery is engaged; a prior boot did not mark success.",
               context={"crash_counter": n},
               remediation=_rec("Inspect the retained history for the failed boot.",
                                "lamboot-inspect history"))]


def rule_no_entries(ctx: Ctx) -> "list[env_mod.Finding]":
    bls = ctx.first("bls_entries_found")
    disc = ctx.first("entries_discovered")
    found_zero = bls is not None and re.search(r"count=0\b", bls.note or "")
    disc_zero = disc is not None and re.search(r"total=0\b", disc.note or "")
    if (bls is None and disc is None) or not (found_zero or disc_zero):
        return []
    return [_f("boot.no_entries", env_mod.SEV_ERROR, "fail",
               "No boot entries were discovered",
               "LamBoot found zero BLS entries to boot.",
               remediation=_rec(
                   "Check BLS placement: /loader/entries on the ESP, with the /boot/ prefix "
                   "for a no-separate-/boot layout.",
                   "lamboot-esp inventory"))]


def rule_firmware_fallback(ctx: Ctx) -> "list[env_mod.Finding]":
    if ctx.events("firmware_loadimage") and not ctx.result.find_loaded_native():
        return [_f("boot.firmware_fallback", env_mod.SEV_WARNING, "warn",
                   "Booted via firmware LoadImage, not the native PE loader",
                   "The native filesystem backend was unavailable for this layout.",
                   remediation=_rec("Confirm the native FS backend covers this /boot filesystem; "
                                    "otherwise a UEFI driver was used."))]
    return []


def rule_measured_skipped(ctx: Ctx) -> "list[env_mod.Finding]":
    for e in ctx.events("kernel_measurement_skipped"):
        if "no_tpm" in (e.note or ""):
            return [_f("trust.measured_skipped", env_mod.SEV_INFO, "info",
                       "Measured boot unavailable (no TPM)",
                       "kernel/cmdline measurement was skipped — this host has no usable TPM.",
                       remediation=_rec("Add a vTPM to enable PCR measurement + attestation."))]
    return []


def rule_kernel_mismatch(ctx: Ctx) -> "list[env_mod.Finding]":
    if ctx.stats.sha256_verify_vs_load_match is False:
        verified = ctx.result.find_verified_load()
        loaded = ctx.result.find_loaded_native()
        return [_f("audit.kernel_mismatch", env_mod.SEV_ERROR, "fail",
                   "Loaded image differs from the verified image",
                   "The sha256 recorded at image_verified does not match image_loaded_native — "
                   "possible corruption or substitution.",
                   context={
                       "verified_sha256": verified.sha256 if verified else None,
                       "loaded_sha256": loaded.sha256 if loaded else None,
                   },
                   remediation=_rec("Re-verify and re-deploy the bootloader/kernel.",
                                    "sudo lamboot-inspect verify"))]
    return []


# Events that are *intrinsically* a boot failure by their name — distinct from a
# degraded-but-successful path (image_verified status=SKIPPED on a SB-off boot is
# NOT a failure; rule_trust_token already characterizes the trust posture).
_EXPLICIT_FAILURE_EVENTS = (
    "image_load_failed",
    "kernel_load_failed",
    "policy_invalid",
    "driver_rejected",
)


def rule_explicit_failures(ctx: Ctx) -> "list[env_mod.Finding]":
    out = []
    for name in _EXPLICIT_FAILURE_EVENTS:
        for e in ctx.events(name):
            out.append(_f("boot.event_failed", env_mod.SEV_ERROR, "fail",
                          f"Failure event: {e.event}",
                          e.note or e.status or "",
                          context={"seq": e.seq, "event": e.event}))
    return out


RULES: "list[Callable[[Ctx], list[env_mod.Finding]]]" = [
    rule_trust_token,
    rule_preflight,
    rule_crashloop,
    rule_no_entries,
    rule_firmware_fallback,
    rule_measured_skipped,
    rule_kernel_mismatch,
    rule_explicit_failures,
]


def evaluate(ctx: Ctx) -> "list[env_mod.Finding]":
    findings: "list[env_mod.Finding]" = []
    for rule in RULES:
        try:
            findings.extend(rule(ctx))
        except Exception:  # a buggy rule must never break diagnosis
            continue
    return findings


def exit_code_for(findings: "list[env_mod.Finding]", *, strict: bool, integrity_violation: bool) -> int:
    """Match the suite's diagnostic convention (lamboot-diagnose:1036):
    critical/error findings -> EXIT_PARTIAL(2); warnings alone stay EXIT_OK(0)
    (the envelope reports status "warn"); a --strict integrity violation is
    EXIT_UNSAFE(4).
    """
    from .cli import EXIT_OK, EXIT_PARTIAL, EXIT_UNSAFE

    if strict and integrity_violation:
        return EXIT_UNSAFE
    if any(f.severity in (env_mod.SEV_ERROR, env_mod.SEV_CRITICAL) for f in findings):
        return EXIT_PARTIAL
    return EXIT_OK


# ---------------------------------------------------------------------------
# Subcommand entry point (called by cli.cmd_diagnose)
# ---------------------------------------------------------------------------


def run(args: argparse.Namespace, resolve_default, emit, io_error) -> int:
    from .cli import EXIT_OK

    path = resolve_default("trust_log", args.path)
    if path is None or not path.exists():
        return io_error(args, f"trust log not found (tried {path or 'ESP auto-detect'})")
    try:
        result = tl.parse(path)
    except (OSError, ValueError) as e:
        return io_error(args, f"cannot read trust log {path}: {e}")

    boot_json = None
    bjpath = resolve_default("boot_json", getattr(args, "boot_path", None))
    if bjpath and bjpath.exists():
        try:
            boot_json = json.loads(bjpath.read_text())
        except (OSError, ValueError):
            boot_json = None

    ctx = Ctx(result=result, stats=tl.summarize(result), boot_json=boot_json)
    findings = evaluate(ctx)

    integrity_violation = bool(result.schema_violations)
    if integrity_violation:
        findings.append(_f("audit.chain_break", env_mod.SEV_WARNING, "warn",
                           "Trust-log integrity issue",
                           f"{len(result.schema_violations)} schema/integrity violation(s) — the "
                           "log may be truncated or altered.",
                           remediation=_rec("Compare against retained history.",
                                            "lamboot-inspect history")))

    floor = _RANK[{"info": env_mod.SEV_INFO, "warn": env_mod.SEV_WARNING,
                   "error": env_mod.SEV_ERROR}[args.severity]]
    shown = [f for f in findings if _RANK[f.severity] >= floor]
    code = exit_code_for(shown, strict=args.strict, integrity_violation=integrity_violation)

    if args.json or args.format == "json":
        data = {"event_count": ctx.stats.total_events, "rules_evaluated": len(RULES)}
        return emit(args, code, shown, data)

    _render_text(shown, args.explain, code)
    return code


def _render_text(findings: "list[env_mod.Finding]", explain: bool, code: int) -> None:
    if not findings:
        sys.stdout.write("✓ clean — no problems found at the requested severity\n")
        return
    n_err = sum(1 for f in findings if f.severity in (env_mod.SEV_ERROR, env_mod.SEV_CRITICAL))
    n_warn = sum(1 for f in findings if f.severity == env_mod.SEV_WARNING)
    verdict = []
    if n_err:
        verdict.append(f"✗ {n_err} problem(s)")
    if n_warn:
        verdict.append(f"⚠ {n_warn} warning(s)")
    sys.stdout.write("  ".join(verdict) + "\n\n")
    sym = {env_mod.SEV_CRITICAL: "✗", env_mod.SEV_ERROR: "✗",
           env_mod.SEV_WARNING: "⚠", env_mod.SEV_INFO: "i"}
    for f in findings:
        sys.stdout.write(f"{sym.get(f.severity, '-')} [{f.severity}] {f.id}: {f.title}\n")
        if f.message:
            sys.stdout.write(f"    {f.message}\n")
        if f.remediation and f.remediation.get("summary"):
            sys.stdout.write(f"    → {f.remediation['summary']}\n")
            if f.remediation.get("command"):
                sys.stdout.write(f"      $ {f.remediation['command']}\n")
        if explain and f.context:
            sys.stdout.write(f"      evidence: {f.context}\n")
        sys.stdout.write("\n")


# Register the subcommand so help/man/website pick it up uniformly.
registry_mod.register(
    registry_mod.Subcommand(
        name="diagnose",
        category="Diagnostics",
        summary="Analyse the last boot and report problems + recommendations",
        syntax="lamboot-inspect diagnose [--path FILE] [--severity info|warn|error] [--explain] [--strict] [--json]",
        args=(
            "--path FILE:Trust log path (default: auto-detect on the ESP)",
            "--severity LEVEL:Minimum severity to report (default: info)",
            "--explain:Include the event evidence each finding derives from",
            "--strict:Treat trust-log integrity violations as UNSAFE (exit 4)",
        ),
        examples=(
            "lamboot-inspect diagnose",
            "lamboot-inspect diagnose --severity warn --json",
            "sudo lamboot-inspect diagnose --explain",
        ),
        notes="Read-only. Maps trust-log events to findings + recommendations. Exit: 0 clean, "
        "2 warnings, 1 problems, 4 an integrity violation under --strict.",
        see_also=("trust-log", "summary", "history"),
        doc_url="https://lamboot.dev/tools/inspect/diagnose",
        maturity="draft",
    )
)
