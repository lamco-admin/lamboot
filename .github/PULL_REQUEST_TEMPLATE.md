<!--
Thanks for contributing to LamBoot. Please fill out the sections below so we
can review efficiently. Items that don't apply can be removed.
-->

## Summary

<!-- One- to three-sentence description of what this PR changes and why. -->

## Type of change

<!-- Pick all that apply. -->

- [ ] Bug fix (does not change user-visible behavior except to fix a defect)
- [ ] Enhancement (changes user-visible behavior or adds capability)
- [ ] Refactor (no behavior change)
- [ ] Documentation
- [ ] Build / packaging / CI
- [ ] Test infrastructure

## Linked issues

<!-- Use "Closes #N" or "Refs #N" so the issue tracker is updated. -->

Closes #

## Testing

<!--
Tell us what you actually exercised. Be specific.
-->

- [ ] `./build.sh` runs clean on x86_64 and aarch64
- [ ] `cargo clippy --workspace --all-targets` is no worse than `main`
- [ ] Booted in QEMU (`./run-qemu.sh`) and the change behaved as expected
- [ ] Cross-distro impact considered (which distros, which filesystems, which Secure Boot states)
- [ ] If the change touches the signed binary or the install script, verified `lamboot-install --signed` still installs cleanly on at least one SB-on guest

## Trust / signing impact

<!--
If this PR touches anything that affects trust evidence (boot.json schema,
trust log events, signing flow, kernel verification path, policy gating),
describe what changes and why.
-->

## Documentation

<!--
LamBoot favors docs alongside code. If you changed user-visible behavior,
update at least one of: README.md, CHANGELOG.md, QUICKSTART.md, docs/.
-->

- [ ] Updated CHANGELOG.md
- [ ] Updated `docs/` where relevant
- [ ] N/A — internal-only change

## Notes for the reviewer

<!-- Anything that's not obvious from the diff. Areas you'd like a closer look. -->
