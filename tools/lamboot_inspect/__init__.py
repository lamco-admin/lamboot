"""lamboot-inspect — LamBoot diagnostic tool.

Parses LamBoot's on-disk diagnostic artefacts (trust log, boot log, policy,
boot reports, EFI variables) and presents them in a form humans and
automation can both consume.

Produced artefacts understood by this tool:

- ``\\loader\\boot-trust.log``    — JSON-lines trust event stream
                                    (schema v2 per SPEC-NATIVE-TRUST-CHAIN §6)
- ``\\EFI\\LamBoot\\reports\\boot.log``   — human-readable boot timeline
- ``\\EFI\\LamBoot\\reports\\boot.json``  — machine-readable last-boot report
- ``\\EFI\\LamBoot\\reports\\audit.log``  — cumulative audit line log
- ``\\EFI\\LamBoot\\policy.toml``         — active boot policy
- EFI variables in the LamBoot GUID namespace

Entry points:

- :mod:`lamboot_inspect.cli`        — argument parsing and dispatch
- :mod:`lamboot_inspect.trust_log`  — schema-aware trust-log parser
- :mod:`lamboot_inspect.boot_log`   — boot-log parser with phase/timing analysis
- :mod:`lamboot_inspect.report`     — boot.json consumer
- :mod:`lamboot_inspect.audit`      — audit.log consumer
- :mod:`lamboot_inspect.verify`     — SPEC-NATIVE-TRUST-CHAIN §8 website-claim
                                   CI checker
- :mod:`lamboot_inspect.dump`       — one-shot bug-report bundle
- :mod:`lamboot_inspect.render`     — text / JSON / timeline renderers

The package is a hard dependency on Python 3.9 or newer. No third-party
packages are required — stdlib only. That keeps ``lamboot-inspect`` usable
in minimal rescue environments where ``pip install`` is not available.
"""

from importlib import metadata

__all__ = ["__version__"]

try:
    __version__ = metadata.version("lamboot-inspect")
except metadata.PackageNotFoundError:
    # Running from a source checkout that hasn't been pip-installed.
    __version__ = "0.8.3"
