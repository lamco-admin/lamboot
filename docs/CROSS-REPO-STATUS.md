# Cross-Repo Coordination Status

**Purpose:** Rolling tracker of coordination items between `lamboot-dev` and
`lamboot-tools-dev` per `~/lamboot-tools-dev/docs/SPEC-LAMBOOT-TOOLKIT-V1.md`
§14.5. (PVE tooling lives in `lamboot-tools-dev/pve/` subtree per founder
decision 2026-04-22; there is no separate companion repo.)

**Mirror counterpart:** `~/lamboot-tools-dev/docs/CROSS-REPO-STATUS.md` — keep
these two files in sync. Owner perspective is flipped between them.

**Review cadence:** Monthly. Update this file at every quarterly
release-planning meeting + whenever a coordination item changes state.

**Last reviewed:** 2026-04-22 (evening — v0.8.4 code sprint landed all
4 must-haves + 3 should-haves; Proxmox integration test still pending)

---

## 0. At-a-glance state

| Repo | Tag | Shipping state | Lead doc |
|---|---|---|---|
| `lamboot-dev` (here) | `v0.8.3` (shipped 2026-04-21) + unreleased `main` @ `2892446` (v0.8.4 prep done) | All 4 v0.8.4 blockers landed; awaits Proxmox integration test before tag | `docs/STATUS-2026-04-22-TOOLKIT-PIVOT.md` + §9 Post-Q appendix |
| `lamboot-tools-dev` | `v0.2.0-rc` (feature-complete, uncommitted) | 5 founder-gated items remaining | `docs/STATUS-2026-04-22-SESSION-HANDOVER.md` |

`lamboot-tools-dev` was AHEAD through this morning. The evening sprint on
this side closed the four coordination must-haves plus three should-haves.
Before `v0.8.4` tags: run the rewritten hookscript + monitor on a real
Proxmox VM on `pve.a.lamco.io`, verify `lamboot-pve-setup doctor-hookscript`
reports OK against the new version, and confirm LamBoot inside the guest
sees the fw_cfg-exposed JSON correctly.

Commits landed this evening:
- `51ce546` — docs sync (CROSS-REPO-STATUS.md, LAMBOOT-TOOLS-OVERVIEW.md, pivot §9, ROADMAP update, SPEC-LAMBOOT-MIGRATE.md §14 RESOLVED, 3 back-links)
- `b812fea` — README "Diagnostic and repair utilities" section
- `c4a9b4e` — `lamboot-install --toolkit-prompt` (+ `--install-toolkit` / `--no-install-toolkit`)
- `ada5cb6` — `lamboot-monitor.py` reads `/etc/lamboot/fleet.toml` `[monitor]`
- `2892446` — `lamboot-hookscript.pl` rewrite to fw_cfg file-reference pattern (v0.8.4)

---

## 1. Active coordination items (v0.2.0 release cycle)

### 1.1 Must have — blocks coordinated v0.8.4 + v0.2.0 release

| Item | Owner here | Status | Notes |
|---|---|---|---|
| Hookscript rewrite to fw_cfg file-reference pattern | `tools/lamboot-hookscript.pl` | 🔄 Landed in `2892446` — integration test pending | Rewritten as schema-compatible writer for `/var/lib/lamboot/<VMID>.json`. Version comment `# version: 0.8.4` lets `lamboot-pve-setup doctor-hookscript` detect it. Unit-tested locally (load_fleet_config / determine_role / write_per_vm_json / character escaping). Proxmox integration test remains before v0.8.4 tags. |
| `/etc/lamboot/fleet.toml` schema consumption | `tools/lamboot-hookscript.pl`, `tools/lamboot-monitor.py` | 🔄 Landed in `2892446` + `ada5cb6` | Both consumers read schema v1 with additive-compatible fallback to hardcoded defaults. Monitor seeds argparse defaults for `--alert-webhook` (HTTPS enforced) and `--log-path`. Hookscript reads `[hookscript]` inject flags + `[roles]`/`[tags]` for role resolution. |
| `lamboot-install --toolkit-prompt` opt-in | `tools/lamboot-install` | ✅ Landed in `c4a9b4e` | `--install-toolkit` / `--no-install-toolkit` flags + interactive `[y/N]` prompt on TTY. Distro-aware install guidance (Fedora/RHEL Copr; Debian/Ubuntu/Arch source tarball for now). Skipped on `--dry-run`, `--update`, `--quiet`, partial failure. |
| README / USER-GUIDE cross-reference to toolkit | `README.md`, `docs/LAMBOOT-TOOLS-OVERVIEW.md` | ✅ Landed in `b812fea` + `51ce546` | README has "Diagnostic and repair utilities" section linking the public repo and this file. `LAMBOOT-TOOLS-OVERVIEW.md` rewritten for 11 tools across three RPM subpackages. |

### 1.2 Should have — important but not blocking

| Item | Owner here | Status | Notes |
|---|---|---|---|
| Cross-reference `KEY-GENERATION.md` → `lamboot-signing-keys` | `docs/KEY-GENERATION.md` | ✅ Landed in `51ce546` | §10 "Operator tooling" section with subcommand list. |
| Cross-reference `SECURE-BOOT-AND-SIGNING-STRATEGY.md` → tool | `docs/SECURE-BOOT-AND-SIGNING-STRATEGY.md` | ✅ Landed in `51ce546` | Operator-tooling section maps `sign-binary`, `rotate`, `verify` subcommands to spec procedures. |
| Cross-reference `OVMF-VARS-PROXMOX.md` → `lamboot-pve-ovmf-vars` | `docs/OVMF-VARS-PROXMOX.md` | ✅ Landed in `51ce546` | §12 notes the mirror relationship; `tools/build-ovmf-vars.sh` remains canonical source. |

### 1.3 Release coordination

| Item | Status | Notes |
|---|---|---|
| Combined release announcement (bootloader v0.8.4 + toolkit v0.2.0 including PVE subpackage) | ⏳ Not drafted | Ship as one coordinated press release; two repos, three RPM subpackages. Toolkit-side draft at `~/lamboot-tools-dev/docs/ANNOUNCEMENTS/v0.2.0.md`. |
| Cross-linked release notes | ⏳ Not drafted | Each repo's CHANGELOG references the other. |
| v0.8.4 path choice documented in `docs/ROADMAP.md` | ⏳ Not started | `RELEASE.md §0.1` in tools-dev defines Path A / B / C. Founder decision 2026-04-22 is Path A (full coordinated v0.8.4) per session picking up after the toolkit sync. |

---

## 2. Canonical source map (§14.2 of toolkit spec)

Files mirrored at release-build time. **Canonical source is authoritative —
never edit the mirror directly.**

| File | Canonical location (this repo) | Mirrored to (tools-dev) | Mirror script |
|---|---|---|---|
| `lamboot-inspect` (Python exec) | `tools/lamboot-inspect` | `tools/lamboot-inspect` | `~/lamboot-tools-dev/publish/mirror-from-lamboot-dev.sh` |
| `lamboot_inspect/` (Python pkg dir) | `tools/lamboot_inspect/` | `tools/lamboot_inspect/` | same |
| `lamboot-inspect.1` (man page) | `tools/lamboot-inspect.1` | `man/lamboot-inspect.1` | same |
| `lamboot-monitor.py` | `tools/lamboot-monitor.py` | `pve/tools/lamboot-pve-monitor` (renamed) | `~/lamboot-tools-dev/publish/mirror-pve-from-lamboot-dev.sh` |
| `build-ovmf-vars.sh` | `tools/build-ovmf-vars.sh` | `pve/tools/lamboot-pve-ovmf-vars` (renamed) | same |
| `lamboot-hookscript.pl` | `tools/lamboot-hookscript.pl` | **NOT mirrored** — documented runtime dependency; users install from this repo's release | N/A |
| `KEY-GENERATION.md` | `docs/KEY-GENERATION.md` | Referenced by toolkit website; not mirrored | N/A |

### 2.1 Mirror verification

Each mirror script writes a `MIRROR-CHECKSUMS.txt` recording sha256 of the
canonical source at mirror time. Toolkit CI clones this repo at the matching
release tag, re-mirrors, and diffs against the checked-in copy.

Most recent mirror runs (see
`~/lamboot-tools-dev/MIRROR-CHECKSUMS.txt` and
`~/lamboot-tools-dev/pve/MIRROR-CHECKSUMS.txt`): 2026-04-23T02:26:15Z against
`v0.8.3` tag.

---

## 3. Schema stability commitments

| Schema | Owner | Consumers here | Stability contract |
|---|---|---|---|
| `/etc/lamboot/fleet.toml` v1 | `lamboot-tools-dev` (toolkit spec §16 Appendix C) | `tools/lamboot-hookscript.pl`, `tools/lamboot-monitor.py` | Additive within v1; breaking changes require `schema_version` bump + coordinated release |
| Per-VM JSON v1 (`/var/lib/lamboot/<VMID>.json`) | `lamboot-tools-dev` (`pve/tools/lamboot-pve-setup` writes at setup-time; hookscript rewrites at pre-start) | LamBoot inside VM (reads via fw_cfg); `tools/lamboot-hookscript.pl` (writes) | Same additive policy |
| Toolkit JSON output schema v1 | `lamboot-tools-dev` (toolkit spec §5) | External consumers; referenced in `docs/LAMBOOT-TOOLS-OVERVIEW.md` | SEMVER-STABLE within major; breaking changes bump toolkit major |
| Trust-log events v2 | `lamboot-dev` (see `docs/specs/SPEC-NATIVE-TRUST-CHAIN.md` §6) | Toolkit's `lamboot-diagnose` + `lamboot-inspect` mirror | Additive tokens OK; renames are major-version event |

---

## 4. Release coordination log

### 4.1 v0.8.4 + v0.2.0 target (coordinated across two repos)

Target window: 2026-Q3

**lamboot-dev v0.8.4** (this repo — the 4 items here gate the coordinated release):

- [x] Hookscript rewrite (fw_cfg file-reference pattern; reads `/etc/lamboot/fleet.toml`) — `2892446`
- [x] `lamboot-monitor.py` reads `/etc/lamboot/fleet.toml` per schema — `ada5cb6`
- [x] `lamboot-install --toolkit-prompt` (+ `--install-toolkit` / `--no-install-toolkit` flags) — `c4a9b4e`
- [x] `README.md` cross-reference; `docs/LAMBOOT-TOOLS-OVERVIEW.md` rewrite — `b812fea` + `51ce546`
- [x] (Should-have) KEY-GENERATION.md / SECURE-BOOT-AND-SIGNING-STRATEGY.md / OVMF-VARS-PROXMOX.md back-links — `51ce546`
- [ ] CHANGELOG entry cross-linking to `lamboot-tools v0.2.0` — not drafted yet
- [ ] Proxmox integration test on `pve.a.lamco.io` with a real VM — pending
- [ ] Rebuild + re-sign + re-tarball `v0.8.4`

**lamboot-tools-dev v0.2.0** (produces three RPM subpackages from one source):

- [x] 9 core tools: migrate / diagnose / esp / backup / repair / doctor / uki-build / signing-keys / toolkit
- [x] PVE subtree: `pve/tools/lamboot-pve-{setup,fleet,monitor,ovmf-vars}`
- [x] `lamboot-inspect` mirror + `lamboot-pve-monitor` + `lamboot-pve-ovmf-vars` mirrors
- [x] `publish/*.sh` scripts (tarball + standalone-migrate + bump-version + mirrors + export-to-public)
- [x] 13 man pages (Session M) + 24 MkDocs website pages (Session N)
- [x] CHANGELOG entry; ANNOUNCEMENTS/v0.2.0.md
- [x] Unified RPM spec (`lamboot-tools.spec` producing `lamboot-tools` + `lamboot-migrate` + `lamboot-toolkit-pve` subpackages); dual-pub `lamboot-migrate-standalone.spec`
- [x] 11 fixture disk images (built 2026-04-22 on pve.a.lamco.io)
- [x] 28/28 release-rehearsal checks; 84/84 verify-claims checks
- [ ] Re-run `mirror-from-lamboot-dev.sh` + `mirror-pve-from-lamboot-dev.sh` against `v0.8.4` tag (currently pinned to `v0.8.3`)
- [ ] Self-hosted GitHub Actions runner on pve registered
- [ ] First Tier 1 fleet-test baseline captured
- [ ] Founder-gated release-runbook execution

---

## 5. Process notes

- **Any item added to this file must have an owner (this repo's module path) and a status.**
- **Status values:** ⏳ Not started / 🔄 In progress / ✅ Done / ⚠️ Blocked / 🗑️ Cancelled.
- **Review cadence:** monthly at minimum; ad-hoc whenever a coordination item changes state.
- **Sync with tools-dev counterpart:** any edit to §1/§2/§3/§4 MUST be mirrored to `~/lamboot-tools-dev/docs/CROSS-REPO-STATUS.md` in the same sitting (owner perspectives flipped). Git blame + recent-reviewed date are the only drift detectors.
- **Escalation path:** items blocked > 30 days surface to founder for direction.
- **Archival:** completed release cycles archived to `docs/archive/cross-repo-status-<release>.md` after release ships.
