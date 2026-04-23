"""Website-claim verifier.

Implements the CI-check portion of SPEC-NATIVE-TRUST-CHAIN §8.3
("Marketing-discipline rules"). Given a checked-out lamboot-dev
repository, walks a table of permitted public claims and confirms
each is backed by a code path or documented SDS line.

The list of permitted claims is authoritative in
``docs/specs/SPEC-NATIVE-TRUST-CHAIN.md`` §8.1 — this module encodes
a machine-readable form with a pointer back to the spec line that
governs it. When SDS-4 §8.1 changes, this table must change in
lockstep; CI diff against the spec file flags drift.

Each check is a ``Claim`` record with:

* ``name``         — short identifier used in CI output
* ``title``        — human-readable version of the claim
* ``description``  — longer explanation
* ``evidence``     — list of file-path + anchor-regex pairs the claim
                     is backed by. Every pair must match for the claim
                     to be considered substantiated.
* ``spec_section`` — pointer back into SPEC-NATIVE-TRUST-CHAIN §8.1
                     for traceability.

Running ``lamboot-inspect verify --website-claims`` walks every claim,
evaluates evidence, and returns exit code 0 if all are substantiated;
nonzero otherwise with a human-readable diff.
"""
from __future__ import annotations

import dataclasses
import pathlib
import re
from typing import Optional


@dataclasses.dataclass
class Evidence:
    path: str
    anchor_regex: str
    description: str = ""


@dataclasses.dataclass
class Claim:
    name: str
    title: str
    description: str
    evidence: list
    spec_section: str


@dataclasses.dataclass
class CheckResult:
    claim: Claim
    substantiated: bool
    missing: list
    matched: list


# ---------------------------------------------------------------------------
# The authoritative claim table. Cross-referenced with
# SPEC-NATIVE-TRUST-CHAIN §8.1 ("Permitted claims (v1.0)").
# ---------------------------------------------------------------------------

CLAIMS: "list[Claim]" = [
    Claim(
        name="fs_backend_trait_exists",
        title="LamBoot has a pluggable Layer-2 filesystem abstraction",
        description=(
            "SPEC-FS-BACKEND-TRAIT defines the FsBackend trait that every "
            "backend implements. The FAT adapter and ext4 skeleton live "
            "on top of it."
        ),
        evidence=[
            Evidence(
                "lamboot-core/src/fs_backend.rs",
                r"pub\(crate\)\s+trait\s+FsBackend",
                "FsBackend trait definition",
            ),
            Evidence(
                "lamboot-core/src/fs_backend_fat.rs",
                r"impl\s+FsBackend\s+for\s+FatBackend",
                "FAT adapter impl",
            ),
            Evidence(
                "lamboot-core/src/fs_backend_ext4.rs",
                r"impl\s+FsBackend\s+for\s+Ext4Backend",
                "ext4 skeleton impl (SDS-2 swap-in point)",
            ),
        ],
        spec_section="SPEC-FS-BACKEND-TRAIT §2",
    ),
    Claim(
        name="esp_writer_fat_only",
        title="Writes to the ESP are FAT-only by construction",
        description=(
            "EspWriter::new returns None unless the underlying Volume's "
            "backend is FAT. This makes 'no ext4 writes ever' a compile-"
            "time property."
        ),
        evidence=[
            Evidence(
                "lamboot-core/src/fs_writer.rs",
                r"if\s+volume\.backend_tag\(\)\s*!=\s*FatBackend::TAG",
                "FAT-only gate in EspWriter::new",
            ),
        ],
        spec_section="SPEC-FS-BACKEND-TRAIT §6.4",
    ),
    Claim(
        name="trust_log_exists",
        title="Every trust decision is recorded in a boot-trust log",
        description=(
            "lamboot-core/src/trust_log.rs writes JSON-lines records to "
            "/loader/boot-trust.log on the ESP. Every load-bearing step "
            "produces an event."
        ),
        evidence=[
            Evidence(
                "lamboot-core/src/trust_log.rs",
                r"pub\(crate\)\s+struct\s+TrustEvent",
                "TrustEvent definition",
            ),
            Evidence(
                "lamboot-core/src/trust_log.rs",
                r"const\s+LOG_PATH.*boot-trust\.log",
                "Canonical log path",
            ),
        ],
        spec_section="SPEC-NATIVE-TRUST-CHAIN §6",
    ),
    Claim(
        name="trust_log_stable_tokens",
        title="Trust-log vocabulary is SEMVER-stable",
        description=(
            "The verified_via tokens are documented in SPEC-NATIVE-"
            "TRUST-CHAIN §6.3 and exercised by the host test suite."
        ),
        evidence=[
            Evidence(
                "lamboot-core/src/fs_backend.rs",
                r"as_log_token",
                "FsError::as_log_token stable tokens",
            ),
            Evidence(
                "tools/lamboot_inspect/trust_log.py",
                r"VERIFIED_VIA_TOKENS",
                "Stable vocabulary in the diagnostic tool",
            ),
        ],
        spec_section="SPEC-NATIVE-TRUST-CHAIN §6.3",
    ),
    Claim(
        name="shim_retain_protocol",
        title="LamBoot asks shim 15.8+ to retain ShimLock across driver loads",
        description=(
            "Before loading any filesystem drivers, LamBoot sets the "
            "ShimRetainProtocol variable (shim #444 / sd-boot precedent) "
            "so ShimLock::Verify remains available for kernel verification."
        ),
        evidence=[
            Evidence(
                "lamboot-core/src/security_override.rs",
                r"request_shim_retain_protocol",
                "request_shim_retain_protocol function",
            ),
            Evidence(
                "lamboot-core/src/main.rs",
                r"request_shim_retain_protocol\(\)",
                "Called before driver loads",
            ),
        ],
        spec_section="SPEC-NATIVE-TRUST-CHAIN §3.2",
    ),
    Claim(
        name="kernel_measured_to_tpm",
        title="Kernel images are measured into TPM PCR 4 before execution",
        description=(
            "lamboot-core/src/boot.rs calls TpmContext::measure_kernel "
            "before start_image, producing a PCR 4 extension event."
        ),
        evidence=[
            Evidence(
                "lamboot-core/src/boot.rs",
                r"tpm\.measure_kernel",
                "measure_kernel call before start",
            ),
            Evidence(
                "lamboot-core/src/tpm.rs",
                r"fn\s+measure_kernel",
                "measure_kernel implementation",
            ),
        ],
        spec_section="SPEC-NATIVE-TRUST-CHAIN §4.3",
    ),
    Claim(
        name="cmdline_measured_to_tpm",
        title="Kernel command lines are measured into TPM PCR 12",
        description=(
            "For BLS + UKI entries with kernel command-line options, "
            "LamBoot extends PCR 12 with the exact options bytes before "
            "handoff."
        ),
        evidence=[
            Evidence(
                "lamboot-core/src/main.rs",
                r"tpm\.measure_cmdline",
                "measure_cmdline call site",
            ),
            Evidence(
                "lamboot-core/src/tpm.rs",
                r"fn\s+measure_cmdline",
                "measure_cmdline implementation",
            ),
        ],
        spec_section="SPEC-NATIVE-TRUST-CHAIN §4.4",
    ),
    Claim(
        name="policy_measured_to_tpm",
        title="Boot policy is measured into TPM PCR 5",
        description=(
            "LamBoot's policy.toml is fed to TpmContext::measure_config "
            "immediately after load, so an attacker modifying policy "
            "cannot escape TPM witness."
        ),
        evidence=[
            Evidence(
                "lamboot-core/src/main.rs",
                r"tpm\.measure_config",
                "measure_config call site (policy.toml)",
            ),
            Evidence(
                "lamboot-core/src/tpm.rs",
                r"fn\s+measure_config",
                "measure_config implementation",
            ),
        ],
        spec_section="SPEC-NATIVE-TRUST-CHAIN §4.2",
    ),
    Claim(
        name="rust_no_std",
        title="LamBoot is a Rust bootloader (no_std, no_main)",
        description=(
            "Every UEFI binary in the workspace is compiled with #![no_std] "
            "and #![no_main]. No C code in the hot path."
        ),
        evidence=[
            Evidence(
                "lamboot-core/src/main.rs",
                r"^#!\[no_main\]",
                "lamboot-core main is no_main",
            ),
            Evidence(
                "lamboot-core/src/main.rs",
                r"^#!\[no_std\]",
                "lamboot-core main is no_std",
            ),
        ],
        spec_section="SPEC-NATIVE-TRUST-CHAIN §1.3",
    ),
]


def check_claim(repo_root: pathlib.Path, claim: Claim) -> CheckResult:
    matched: list = []
    missing: list = []
    for ev in claim.evidence:
        path = repo_root / ev.path
        if not path.exists():
            missing.append(ev)
            continue
        try:
            content = path.read_text(encoding="utf-8", errors="replace")
        except OSError:
            missing.append(ev)
            continue
        if re.search(ev.anchor_regex, content, re.MULTILINE):
            matched.append(ev)
        else:
            missing.append(ev)
    substantiated = len(missing) == 0 and bool(claim.evidence)
    return CheckResult(claim=claim, substantiated=substantiated, missing=missing, matched=matched)


def check_all(repo_root: pathlib.Path) -> "list[CheckResult]":
    return [check_claim(repo_root, c) for c in CLAIMS]
