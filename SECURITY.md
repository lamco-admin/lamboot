# Security Policy

## Reporting a vulnerability

If you believe you've found a security vulnerability in LamBoot, please report it privately via email to **security@lamco.io** (preferred) or **office@lamco.io**. We'd like a responsible-disclosure process:

1. Email us a description of the issue, affected versions, and any proof-of-concept.
2. GPG-encrypt your email with our key (fingerprint published on the project website once v0.8.3 ships).
3. We aim to acknowledge within 48 hours.
4. We coordinate with you on disclosure timing — typically 60-90 days to allow patching in releases downstream.
5. We credit reporters in the release notes unless you prefer anonymity.

## What counts as a security issue

**In scope:**
- Memory corruption in LamBoot's Rust code (even in `unsafe` blocks)
- Signature verification bypasses or logic errors in LamBoot-specific code
- MOK enrollment flaws in `lamboot-install`
- Trust-evidence log tampering vulnerabilities (beyond the advisory v0.8.3 limitation — see [`docs/SECURITY-MODEL.md`](docs/SECURITY-MODEL.md))
- TPM measurement gaps or incorrect PCR extensions
- Filesystem driver invocation flaws specific to LamBoot
- Information disclosure of signing keys or MOK passwords through LamBoot code paths

**Out of scope (report upstream):**
- Bugs in `shim`, `grub`, the Linux kernel, OVMF, UEFI firmware, EfiFs drivers, or other dependencies. Report those to their respective projects.
- The class of "bootloader → kernel handoff is not verified" — this is an ecosystem-wide design reality documented in our [security model](docs/SECURITY-MODEL.md) §3. Not a bug in LamBoot.
- Secure Boot disabled by user (out of scope for any bootloader).

## Supported versions

Only the latest released LamBoot version receives security updates. We aim for a monthly maintenance cadence during v0.x; post-v1.0 will adopt a formal N/N-1 support window.

| Version | Supported |
|---|---|
| 0.8.3 (current) | ✅ |
| 0.2.x and earlier | ❌ Pre-release; no security support |

## Signing keys

See [`docs/KEY-GENERATION.md`](docs/KEY-GENERATION.md) for production key hierarchy (PK/KEK RSA 4096, db RSA 2048 per [RSA-4096 compatibility analysis](docs/analysis/RSA-4096-COMPATIBILITY-ANALYSIS-2026-04-20.md)).

Public certificates are distributed in release tarballs as `keys/db.der` for MOK enrollment.

**Key rotation policy:**
- PK/KEK: 10-year validity; rotated only on key compromise or scheduled expiry
- db: 3-year validity; rotation planned for 2029 via KEK re-signing

If the db signing key is compromised, expect an emergency release with a new db cert and revocation (dbx entry) for the old cert. Subscribe to release announcements for advisories.

## What LamBoot does differently

LamBoot is written in Rust with `unsafe` encapsulated in audited primitives. We maintain a trust-evidence log (`\loader\boot-trust.log`) that surfaces every image-authentication decision — no other production Linux bootloader does this. See [`docs/SECURITY-MODEL.md`](docs/SECURITY-MODEL.md) for the full threat model and our honest statement of what we defend against vs. what remains in the broader ecosystem.
