# lamboot-inspect — diagnostic tool

`lamboot-inspect` is LamBoot's Swiss-army-knife diagnostic tool. It reads every
artefact LamBoot writes to the ESP, renders it for a human or an automation
pipeline, and verifies LamBoot's public claims against the source of the
installation it's examining.

The tool is strictly read-only. It never modifies system state, never touches
EFI variables except to read them, and never uploads anything.

## Installation

`lamboot-inspect` ships with every LamBoot release tarball under `tools/`:

```
tools/
├── lamboot-inspect              # entry-point script
├── lamboot_inspect/             # implementation package
├── lamboot-inspect.1            # man page
└── completions/
    ├── lamboot-inspect.bash     # bash completion
    └── _lamboot-inspect         # zsh completion
```

Install system-wide:

```bash
sudo install -m 0755 tools/lamboot-inspect /usr/local/bin/lamboot-inspect
sudo cp -r tools/lamboot_inspect /usr/local/lib/
sudo install -m 0644 tools/lamboot-inspect.1 /usr/local/share/man/man1/
sudo install -m 0644 tools/completions/lamboot-inspect.bash \
    /etc/bash_completion.d/
```

Or run directly from the release tarball — the script looks for its support
package in the adjacent `lamboot_inspect/` directory.

Only dependency: Python 3.9+. No `pip install` required.

## Quick start

**Summary of the last boot:**
```
$ lamboot-inspect summary
```

**Show me every trust decision from the last boot:**
```
$ lamboot-inspect trust-log
```

**Everything that went wrong:**
```
$ lamboot-inspect trust-log --errors-only
```

**Timing breakdown of the boot:**
```
$ lamboot-inspect boot-log
```

**File a bug report:**
```
$ lamboot-inspect dump
# produces lamboot-inspect-dump-YYYYMMDD-HHMMSS.tar.gz
```

## Subcommand reference

### `trust-log`

Parses the JSON-lines trust log at `/loader/boot-trust.log` (schema v2 per
`SPEC-NATIVE-TRUST-CHAIN.md` §6) and renders events in one of four modes:

| Format     | Purpose                                                   |
|------------|-----------------------------------------------------------|
| `text`     | Human-readable, coloured. Default.                        |
| `json`     | Machine-readable, suitable for `jq`/scripts.              |
| `timeline` | Compact sparkline. One event per line, ≤80 cols.          |
| `stats`    | Aggregate counts + verify↔load SHA-256 invariant check.   |

Options:
- `-p`, `--path`  — override the trust-log path (default: auto-detect ESP)
- `-e`, `--event` — filter to one event name (e.g. `image_verified`)
- `--errors-only` — show only failures and degraded-trust paths
- `--no-sha`      — omit SHA-256 columns (narrower output)
- `--strict`      — exit 4 if any records fail schema v2 validation

**Exit codes:** 0 on clean parse, 2 on file not readable, 3 on total parse
failure, 4 on schema violation (with `--strict`).

### `boot-log`

Parses `/EFI/LamBoot/reports/boot.log` — the human-readable boot trace
written by `bootlog.rs`. Supports phase-timing extraction and level filtering.

Options:
- `-p`, `--path`  — override path
- `-l`, `--level` — filter by level (`DEBUG`, `INFO`, `WARN`, `ERROR`)
- `--errors-only` — show only WARN + ERROR
- `-f`, `--format` — `text` or `json`

### `summary`

Produces a one-page view of the last boot. Pulls:
- `boot.json` (machine-readable report)
- Last boot-trust events (verify, load, attempt)
- Recent audit log entries
- Any warnings from the boot log

Useful as the first command to run when investigating a boot issue.

### `show`

Shows full detail for one trust event, looked up by:
- Sequence number (`show 7`)
- Event name (`show image_verified` — returns first match)
- Image path (`show /boot/vmlinuz-6.12`)

### `verify`

Walks the SDS-4 §8.1 permitted-claims table and checks each is backed by
code-path evidence in the checkout. Intended for CI and for auditors who
want to trace a public claim back to its code.

Each claim lists a spec section (e.g. `SPEC-FS-BACKEND-TRAIT §6.4`) so a
reviewer can confirm the claim matches the authoritative source.

Exit code 5 if any claim is not substantiated. The tool prints the missing
evidence anchors so the fix is actionable.

### `dump`

Produces a tar.gz diagnostic bundle containing:

- `trust/boot-trust.log`
- `reports/boot.log`, `boot.json`, `audit.log`, `error.json`
- `config/policy.toml`, `manifest.toml`
- `efivars/<LamBoot-namespace vars>` (if `efivarfs` is mounted)
- `system/` — `uname`, `os-release`, `cmdline`, `lsblk`, `blkid`,
  `efibootmgr`, `mokutil` output
- `manifest.sha256` — SHA-256 of every included file
- `collection.log` — timeline and any skipped items with reasons

Best-effort: missing tools (`efibootmgr`, `mokutil`) are noted in
`collection.log` rather than failing the bundle.

Attach the resulting tar.gz to a GitHub issue.

## Exit codes

| Code | Meaning                                                           |
|------|-------------------------------------------------------------------|
| 0    | Success                                                           |
| 1    | Usage error (unknown subcommand, bad argument)                    |
| 2    | Source file unreadable                                            |
| 3    | Parse error                                                       |
| 4    | Schema violation (`--strict` only)                                |
| 5    | Verification failed                                               |
| 6    | Dump collection failed                                            |

## Environment variables

- `NO_COLOR`                   — disable ANSI colour output
- `LAMBOOT_DIAG_FORCE_COLOR=1` — force colour even when piped

## CI integration

The `verify` subcommand is designed to run in CI. Example GitHub Actions
step:

```yaml
- name: Verify LamBoot website-claim evidence
  run: lamboot-inspect verify --repo . --verbose
```

Exit code 5 fails the workflow.

## Development

Running the test suite:

```bash
python3 -m pytest tools/tests/ -v
```

No external dependencies. Tests use pytest's fixture collection from
`tools/tests/fixtures/`.
