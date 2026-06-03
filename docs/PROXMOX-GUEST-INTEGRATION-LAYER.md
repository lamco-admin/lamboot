# LamBoot Proxmox Guest-Integration Layer

**Status:** working / shipping
**LamBoot version covered:** 0.11.15
**Hookscript version covered:** 0.8.4 (`tools/lamboot-hookscript.pl`)
**Monitor version covered:** current `tools/lamboot-monitor.py`
**Date:** 2026-05-28
**Audience:** Proxmox operator running a fleet of LamBoot-booted guest VMs

---

## 1. Purpose & scope

This layer is the **host-side software on a Proxmox VE node that interacts
with LamBoot-running guest VMs**. It is distinct from — and orthogonal to
— installing LamBoot as the **host node's** own bootloader (that is the
PATH A/B/C work described in `proxmox-host-install/`).

What this layer does:

1. **Injects per-VM, per-fleet metadata into each guest** at VM start via
   QEMU's `fw_cfg` interface, so LamBoot inside the guest can read its
   identity (VMID, fleet ID, role) without any agent inside the guest.
2. **Captures boot-health data from each guest's UEFI variables** at VM
   stop, so the host has a fleet-wide rolling record of LamBoot's
   self-reported `LamBootState`, `LamBootCrashCount`, `LamBootLastEntry`,
   `LamBootTimestamp`, and `LamBootVersion`.
3. **Surfaces crash-loop detection** at the host log layer so an
   operator can act on a CrashLoop guest before the next boot.

What this layer is **not**:

- Not a LamBoot-on-host install.
- Not a guest OS agent (the guest does not need to know this layer exists).
- Not a cluster-wide service; each Proxmox node runs its own instance,
  reading its own `/etc/lamboot/fleet.toml`, watching only the VMs hosted
  on that node.

---

## 2. Architecture

```
┌──────────────────────────────────────────────────────────────────────┐
│ Proxmox host (e.g. pve2)                                             │
│                                                                      │
│  /etc/lamboot/fleet.toml         ← operator-edited; v1 schema        │
│           │                                                          │
│           ▼                                                          │
│  /var/lib/vz/snippets/                                               │
│  lamboot-hookscript.pl           ← invoked by qm at each lifecycle   │
│           │                                                          │
│   pre-start → reads fleet.toml, reads /etc/pve/qemu-server/<vmid>    │
│               .conf, writes /var/lib/lamboot/<vmid>.json,            │
│               appends snapshot to fleet.jsonl                        │
│   post-stop → invokes lamboot-monitor.py on the just-stopped VM,     │
│               captures boot health from OVMF_VARS, appends to        │
│               fleet.jsonl                                            │
│           │                                                          │
│           ▼                                                          │
│  /var/lib/lamboot/<vmid>.json    ← exposed to guest via fw_cfg       │
│  /var/log/lamboot/hookscript.log ← human-readable audit trail        │
│  /var/log/lamboot/fleet.jsonl    ← structured rolling boot-health log│
└────────────────────────────────┬─────────────────────────────────────┘
                                 │
                                 │ qm starts the VM, QEMU exposes:
                                 │   -fw_cfg name=opt/lamboot/config,
                                 │           file=/var/lib/lamboot/<vmid>.json
                                 ▼
┌──────────────────────────────────────────────────────────────────────┐
│ Guest VM (e.g. 364)                                                  │
│                                                                      │
│  LamBoot reads fw_cfg blob at startup                                │
│   → identifies its own VMID + fleet ID + role                        │
│   → renders these in the GUI menu header                             │
│   → logs them in /boot/EFI/LamBoot/reports/boot.json                 │
│                                                                      │
│  LamBoot also writes to its UEFI variables (vendor GUID              │
│  4c414d42-4f4f-5400-0000-000000000001):                              │
│    LamBootState         (u8: 0=Fresh, 1=Booting, 2=BootedOK, 3=Crash)│
│    LamBootCrashCount    (u8)                                         │
│    LamBootLastEntry     (UTF-8)                                      │
│    LamBootTimestamp     (epoch seconds)                              │
│    LamBootVersion       (u32 packed)                                 │
│                                                                      │
│  (Persisted in the VM's efidisk0 OVMF_VARS file across reboots.)     │
└──────────────────────────────────────────────────────────────────────┘
```

Two host-side processes do real work; one is a Perl hookscript driven
by Proxmox's lifecycle hooks, the other is a Python boot-health reader
called from inside the hookscript. Everything else is plain config files
and append-only logs.

---

## 3. Components inventory

| File on host | Source in dev repo | Mode | Purpose |
|---|---|---|---|
| `/var/lib/vz/snippets/lamboot-hookscript.pl` | `tools/lamboot-hookscript.pl` | 0755 | Proxmox `--hookscript` target; called as `<vmid> <phase>` |
| `/usr/local/bin/lamboot-monitor.py` | `tools/lamboot-monitor.py` | 0755 | Reads OVMF_VARS, emits JSON boot-health record per VM |
| `/etc/lamboot/fleet.toml` | (operator-authored, schema v1) | 0644 | Fleet identity + per-VM role assignment + per-field injection toggles |
| `/var/lib/lamboot/` | (created at install) | 0755 dir | Per-VM JSON state files + any staged certs/markers |
| `/var/lib/lamboot/<vmid>.json` | (rewritten on every pre-start) | 0644 | Output to guest via fw_cfg |
| `/var/log/lamboot/` | (created at install) | 0755 dir | Logs |
| `/var/log/lamboot/hookscript.log` | (appended by hookscript) | 0644 | Human-readable timestamped lifecycle audit |
| `/var/log/lamboot/fleet.jsonl` | (appended by hookscript + monitor) | 0644 | Structured boot-health records, one JSON object per line |

No systemd units. No daemons. The hookscript runs only when Proxmox
invokes it; the monitor runs only when the hookscript calls it.

Dependencies installed by Proxmox by default that the layer relies on:
`perl >= 5.30`, `python3 >= 3.11` (for `tomllib`; on older Proxmox where
3.11+ isn't default, `tomli` is the fallback), `qemu-nbd` (only used by
`lamboot-monitor.py` to mount OVMF_VARS images), and `pvesm` (Proxmox
storage tool, for resolving `efidisk0` storage spec to a file path).

---

## 4. Installation procedure

Idempotent. Re-running these steps overwrites in place; no `_old.bak`
files accumulate.

### 4.1 Copy the host-side files

```bash
# As root on the Proxmox node:
install -Dm0755 lamboot-hookscript.pl /var/lib/vz/snippets/lamboot-hookscript.pl
install -Dm0755 lamboot-monitor.py    /usr/local/bin/lamboot-monitor.py
install -d -m0755 /etc/lamboot /var/lib/lamboot /var/log/lamboot
```

The hookscript MUST live under `/var/lib/vz/snippets/` for Proxmox to
resolve `local:snippets/lamboot-hookscript.pl`. Storing it elsewhere
breaks `qm set --hookscript ...` lookup.

### 4.2 Verify prerequisites

```bash
perl -c /var/lib/vz/snippets/lamboot-hookscript.pl   # → "syntax OK"
python3 -c "import tomllib"                          # → no error on 3.11+
python3 -m py_compile /usr/local/bin/lamboot-monitor.py
```

If `python3 -c "import tomllib"` fails (older Python on Proxmox 7.x),
install the `python3-tomli` apt package; the hookscript automatically
falls back.

### 4.3 Author `/etc/lamboot/fleet.toml`

See §5 for the full schema. The minimum useful config is:

```toml
schema = "v1"

[fleet]
id = "lamco-pve2"
description = "pve2.a.lamco.io test fleet"

[roles]
"364" = "lamboot-archinstall-target"
```

### 4.4 Attach the hookscript + `fw_cfg` args to each LamBoot guest

For each guest VMID `<N>` where LamBoot is the bootloader:

```bash
qm set <N> --hookscript local:snippets/lamboot-hookscript.pl
qm set <N> --args "-fw_cfg name=opt/lamboot/config,file=/var/lib/lamboot/<N>.json"
```

`qm set --args` is **append-only-by-the-operator**: re-running with a
different value replaces the whole `args` line. If the VM already had
other QEMU CLI args, snapshot the existing value first and concatenate:

```bash
EXISTING=$(qm config <N> | sed -n 's/^args: //p')
FWCFG="-fw_cfg name=opt/lamboot/config,file=/var/lib/lamboot/<N>.json"
qm set <N> --args "${EXISTING:+${EXISTING} }${FWCFG}"
```

**Important:** `qm set --args` takes effect on the **next start** of the
VM, because QEMU receives its command line at process exec time. A
running VM keeps its existing `-fw_cfg` (or lack of one) until cold-stopped
and restarted; `qm reboot` is not enough — it reboots the guest OS inside
the same QEMU process. Use `qm stop <N> && qm start <N>` to pick up new
args.

### 4.5 Optionally seed an initial JSON state file

Without this, the first VM start that picks up the new args will have
`/var/lib/lamboot/<N>.json` written by the hookscript's `pre-start` 1
second before QEMU's fw_cfg open call — that is fine in practice but is
visible-to-an-impatient-debugger as a brief race. To eliminate it:

```bash
/var/lib/vz/snippets/lamboot-hookscript.pl <N> pre-start
```

This is the same code path Proxmox triggers; running it manually is
idempotent and writes the JSON exactly as the next real pre-start would.

---

## 5. `/etc/lamboot/fleet.toml` schema v1

Authoritative source: the parser in `lamboot-hookscript.pl` (lines
~165–207 for the loader, ~230–254 for role determination). The Python
side (`lamboot-monitor.py`) reads the same file but only uses `[fleet].id`.

### 5.1 Top-level

```toml
schema = "v1"             # REQUIRED. Hookscript silently returns {} if mismatched.
                          # Allowed values: "v1" (string) OR omitted entirely.
                          # If you set schema = "v2" or anything else, the
                          # hookscript treats the file as empty.
```

### 5.2 `[fleet]` — fleet identity (REQUIRED to populate `fleet_id` in JSON)

```toml
[fleet]
id          = "lamco-pve2"            # Free-form; surfaces in guest JSON + logs
description = "pve2 test fleet"       # Optional; not surfaced to guest
```

### 5.3 `[roles]` — explicit VMID → role-name map

```toml
[roles]
"364" = "lamboot-archinstall-target"
"365" = "lamboot-archinstall-target"
```

Keys are **VMID strings** (quoted). Values are arbitrary role names. An
explicit `[roles]` entry **wins over** tag-based matching (§5.4).
Per-VM, the role appears in the per-VM JSON as `"role": "<value>"` and
in `fleet.jsonl` as the `role` field of the monitor record (if the
monitor is run later for that VM — currently the monitor doesn't read
the role table, it only writes from the OVMF_VARS readback).

### 5.4 `[tags]` — tag-based role assignment (fallback when `[roles]` doesn't list the VM)

```toml
[tags]
"lamboot-target"       = ["lamboot", "uefi-test"]
"production-customer"  = ["prod", "customer"]
```

The hookscript reads `tags:` from `/etc/pve/qemu-server/<vmid>.conf`
(Proxmox's per-VM tags field; semicolon- or comma-separated), and for
each role name in `[tags]` (sorted alphabetically for determinism), checks
if any of its tag list appears in the VM's tags. First match wins.

This is useful when you have many VMs and don't want to enumerate them
all in `[roles]`. **Tag matching is bypassed entirely if the VM has an
explicit `[roles]` entry.**

### 5.5 `[hookscript]` — per-field injection toggles (default: all on)

```toml
[hookscript]
inject_vmid     = true   # Default: true. False → omit "vmid" from JSON.
inject_fleet_id = true   # Default: true. False → omit "fleet_id" from JSON.
inject_role     = true   # Default: true. False → omit "role" from JSON + skip role determination.
```

If **all three** are `false`, the hookscript logs `"All inject_* flags
disabled in fleet.toml [hookscript]; skipping JSON refresh"` and writes
nothing — the guest then sees whatever was in the JSON file from the
previous run (or no file at all on first start). Set all three to `true`
or omit the `[hookscript]` table to use the default-all-on behavior.

### 5.6 Schema validation

The hookscript does **best-effort** validation:

- TOML parse failure → empty config → `role`, `fleet_id` come back null/empty.
- `schema = "v2"` or any non-v1 value → empty config (same as above).
- Missing `[fleet]` → `fleet_id` field in JSON is `null` but JSON is still written.
- Missing `[roles]` AND `[tags]` → `role` field in JSON is `null`.
- Malformed `[roles]` (not a hash) → silently ignored, falls through to tag matching.

No errors are surfaced to the operator beyond `/var/log/lamboot/hookscript.log`.
Use `journalctl -u pve-cluster -n 50` or check the hookscript.log to
diagnose; the script never blocks a VM start, by design.

---

## 6. Per-VM JSON schema v1 (what the guest sees)

Path on host: `/var/lib/lamboot/<VMID>.json`
Exposed to guest at: QEMU fw_cfg blob `opt/lamboot/config`
Read by guest from: `/sys/firmware/qemu_fw_cfg/by_name/opt/lamboot/config/raw`
                    (after the guest kernel mounts `qemu_fw_cfg`; LamBoot reads
                    via the FwCfg DMA interface directly, before any kernel)

### Example (with all injects on):

```json
{
  "schema_version": "v1",
  "vmid": "364",
  "hostname": "pve2",
  "fleet_id": "lamco-pve2",
  "role": "lamboot-archinstall-target",
  "written_by": "lamboot-hookscript 0.8.4",
  "written_at": "2026-05-29T00:20:58Z",
  "tags_at_setup": []
}
```

### Field semantics

| Field | Type | Always present? | Source |
|---|---|---|---|
| `schema_version` | string `"v1"` | yes | hardcoded in hookscript |
| `vmid` | string | only if `inject_vmid` (default on) | VMID passed as `$ARGV[0]` |
| `hostname` | string | yes | `hostname -s` from POSIX uname |
| `fleet_id` | string or `null` | only if `inject_fleet_id` | `[fleet].id` from fleet.toml |
| `role` | string or `null` | only if `inject_role` | `[roles]."<VMID>"` or tag match |
| `written_by` | string | yes | `lamboot-hookscript $HOOKSCRIPT_VERSION` |
| `written_at` | RFC 3339 UTC | yes | `strftime("%Y-%m-%dT%H:%M:%SZ", gmtime)` |
| `tags_at_setup` | array of strings | yes | `tags:` from `/etc/pve/qemu-server/<vmid>.conf` at pre-start time |

`tags_at_setup` is intentionally the **VM's** tag list (from its Proxmox
config), NOT the role-tag list from `fleet.toml`. The name reflects that
these are the operator-set tags at the moment of this pre-start.

---

## 7. Lifecycle event matrix

Proxmox calls the hookscript with two positional args: `<vmid>` and
`<phase>`. The hookscript dispatches on phase:

| Phase | When | Hookscript work | Side effects |
|---|---|---|---|
| `pre-start` | After config validation, before QEMU exec | Refresh `/var/lib/lamboot/<vmid>.json`; call lamboot-monitor.py to capture **previous boot's** health (because pflash file is still readable; OVMF_VARS hasn't been touched yet by this boot) | New JSON, log line + JSONL line. If previous boot was CrashLoop, log line includes `WARNING`. |
| `post-start` | After QEMU is alive and responsive | Log line only ("VM started") | log line |
| `pre-stop` | Before graceful stop | **No-op.** Included for completeness. | none |
| `post-stop` | After QEMU has exited | Call lamboot-monitor.py to capture the **just-completed boot's** health (OVMF_VARS at last-flush state) | log line + JSONL line |

`post-start` is intentionally a no-op for now — there is no use case yet
that requires firing on the QEMU-alive transition. Reserve it.

Note the asymmetry between pre-start and post-stop monitor invocations:

- **pre-start monitor reads** = "the boot we just left ended like X."
  If you boot, then `qm stop`, then `qm start` again, the pre-start of
  the second `qm start` reads the same flush that `qm stop`'s post-stop
  already captured. That's the design — pre-start gives you a fresh-from-
  pflash readout in case the previous post-stop missed (host crash,
  abrupt termination).
- **post-stop monitor reads** = "this boot ended like X." This is the
  canonical signal; the pre-start one is the safety net.

---

## 8. fw_cfg interface (host ↔ guest contract)

QEMU's fw_cfg lets the host expose arbitrary blobs to the guest at known
selector names. The host side:

```
qm set <N> --args "-fw_cfg name=opt/lamboot/config,file=/var/lib/lamboot/<N>.json"
```

becomes, in the QEMU command line:

```
... -fw_cfg name=opt/lamboot/config,file=/var/lib/lamboot/<N>.json ...
```

QEMU reads the file at process start and exposes it under the fw_cfg
selector `opt/lamboot/config`. The `opt/` prefix is the QEMU-reserved
namespace for guest-OS-supplied configuration (guaranteed not to collide
with QEMU's own fw_cfg entries).

### How the guest accesses it

**Before kernel:** LamBoot reads via the UEFI FwCfg protocol exposed by
OVMF (the firmware that backs the VM). OVMF probes the host's QEMU
fw_cfg device on every boot; if `opt/lamboot/config` is present, LamBoot
will see it and can parse the JSON.

**After kernel:** The Linux kernel exposes fw_cfg at
`/sys/firmware/qemu_fw_cfg/by_name/opt/lamboot/config/raw` — `cat`
that file inside the running guest to see exactly what LamBoot would
have read at boot.

### Why fw_cfg and not SMBIOS

An earlier hookscript version (pre-0.8.4) injected metadata via SMBIOS
strings using `qm set --smbios=...`. Two problems:
- `qm set` requires the VM config lock, which Proxmox holds during the
  pre-start lifecycle event, deadlocking.
- SMBIOS strings are length-limited and have an awkward enum-of-fields
  interface.

The fw_cfg approach (file specified in `args`, content rewritten via
plain `cat`/`write` on the file path) sidesteps both — no config-lock
contention, blob can be arbitrary size, the host can update it without
touching the VM config at all once the args line is in place.

### Why a file, not a string fw_cfg

QEMU's `-fw_cfg name=NAME,string=VALUE` form embeds the value in the
QEMU command line, which means rewriting it requires `qm set` (back to
the config-lock problem). The `file=PATH` form makes QEMU read the file
at exec time, so the host can rewrite the file's contents between
process exits without touching VM config. We get all-host-side mutability
for free.

---

## 9. `lamboot-monitor.py` operation

Invoked by the hookscript on `pre-start` and `post-stop`. Also runnable
manually: `lamboot-monitor.py [--json] [--alert-webhook URL] [--threshold N]`.

### What it does

1. Scans `/etc/pve/qemu-server/*.conf` for VMs with `bios: ovmf` and an
   `efidisk0:` entry.
2. For each match, resolves the `efidisk0` storage spec to a file path
   via `pvesm path`.
3. If the VM is stopped (or we're called for a stopped VMID), reads the
   OVMF_VARS file directly. If running, uses `qemu-nbd` to safely read
   the variables from the live VARS image without disrupting the VM.
4. Parses the UEFI variable store (varstore.dat) for variables with the
   LamBoot vendor GUID `4c414d42-4f4f-5400-0000-000000000001`:
   - `LamBootState` (u8 enum: 0=Fresh, 1=Booting, 2=BootedOK, 3=CrashLoop)
   - `LamBootCrashCount` (u8)
   - `LamBootLastEntry` (UTF-8 string)
   - `LamBootTimestamp` (epoch seconds)
   - `LamBootVersion` (u32, packed major.minor.patch)
5. Computes an overall `status` field: `"healthy"` (state=BootedOK,
   crash_count below threshold), `"warning"` (crash_count >= threshold),
   `"critical"` (state=CrashLoop).
6. Emits a JSON record per VM (one VM with `--vmid <N>`, or all OVMF
   VMs without filter).

### Output shape

Per VM:

```json
{
  "vmid": 364,
  "name": "arch-btrfs-lbi",
  "state": "BootedOK",
  "crash_count": 0,
  "last_entry": "Arch Linux (7.0.10-arch1-1)",
  "timestamp": "2026-05-29T00:51:34Z",
  "version": "0.11.14",
  "status": "healthy",
  "qmp_status": "running"
}
```

When invoked by the hookscript on `pre-start` / `post-stop`, the JSON
goes to `stdout`; the hookscript reads it and appends it as one line to
`/var/log/lamboot/fleet.jsonl`.

### Caveats

- Reading OVMF_VARS of a **running** VM via `qemu-nbd` requires the VM
  storage to be a block device or qcow2 — works for `local-lvm`,
  `local-zfs`, `local` (qcow2). NFS-backed storage with raw images is
  trickier; the script falls back to refusing the read if `qemu-nbd`
  errors.
- The variable values reflect the last flush by OVMF, which on most
  setups is at every guest write to efivars. There's a small window
  between a guest write and the host-visible flush.

---

## 10. Observability

### `/var/log/lamboot/hookscript.log` — human audit trail

One line per hookscript invocation. Format:

```
[2026-05-28T19:20:58] VM 364 (pre-start): Refreshed /var/lib/lamboot/364.json (fleet=lamco-pve2 role=lamboot-archinstall-target)
[2026-05-28T19:20:58] VM 364 (pre-start): VM starting
[2026-05-28T19:21:03] VM 364 (post-start): VM started
[2026-05-28T19:45:12] VM 364 (post-stop): VM stopped, capturing boot health
[2026-05-28T19:45:13] VM 364 (post-stop): Boot health captured
```

Tail this when debugging. There's no rotation — operator's responsibility
(logrotate snippet not shipped yet; see §13).

### `/var/log/lamboot/fleet.jsonl` — structured rolling log

One JSON object per line. Each line is a monitor record (see §9 output
shape). Append-only. Suitable for ingestion into log-shipping pipelines
(Loki, Vector, etc.) or for `jq` analysis:

```bash
# Show last 10 records for VM 364
grep '"vmid": *364' /var/log/lamboot/fleet.jsonl | tail -10 | jq

# Count crash-loops in the fleet over the log's lifetime
jq -c 'select(.status == "critical")' /var/log/lamboot/fleet.jsonl | wc -l
```

### `/var/lib/lamboot/<vmid>.json` — current-state snapshot

Always exactly the data the guest will see on its next boot. Cheap
to `cat` for debugging:

```bash
cat /var/lib/lamboot/364.json | jq
```

---

## 11. Troubleshooting

### Symptom: `role` is empty in `/var/lib/lamboot/<vmid>.json`

Three causes, in order of frequency:

1. **fleet.toml schema mistake.** The role table is `[roles]
   "364" = "..."` (flat string-keyed), NOT `[vms.364] role = "..."`.
   The hookscript looks at `fleet->{roles}{$vmid}`, not nested tables.
2. **`inject_role = false`** in `[hookscript]`. Either explicitly set
   it to `true` or remove the line (default is on).
3. **TOML parse error.** Run `python3 -c "import tomllib;
   tomllib.load(open('/etc/lamboot/fleet.toml','rb'))"` to surface the
   parse error. The hookscript silently returns an empty config on
   parse failure.

### Symptom: hookscript.log is empty / `qm start` doesn't invoke the script

- Verify `qm config <N> | grep hookscript:` shows the line. If absent,
  rerun the `qm set --hookscript local:snippets/lamboot-hookscript.pl`
  command.
- Verify the script lives at `/var/lib/vz/snippets/lamboot-hookscript.pl`
  exactly (not under a different snippets storage). Proxmox resolves the
  `local:snippets/` prefix against `/var/lib/vz/snippets/` for the
  `local` storage; if your snippets are on a different storage, change
  the `qm set` line to match (`<storage>:snippets/<file>`).
- Verify the script is executable: `ls -la /var/lib/vz/snippets/lamboot-hookscript.pl`
  (should be `-rwxr-xr-x`).

### Symptom: pre-start JSON written but guest LamBoot doesn't see the fleet_id

- Verify the `-fw_cfg ... file=...` argument actually made it into
  the QEMU command line: `ps -ef | grep "kvm -id <N>"` and look for
  `-fw_cfg name=opt/lamboot/config,file=/var/lib/lamboot/<N>.json`.
- If absent, check `qm config <N> | grep args:` and either set the args
  line or fix the existing one.
- **If the VM was already running when you set the args**, the running
  QEMU process does not pick up the new args. `qm reboot` is not enough.
  `qm stop <N> && qm start <N>` is required.
- Inside the booted guest (after kernel), confirm fw_cfg sees the blob:
  `cat /sys/firmware/qemu_fw_cfg/by_name/opt/lamboot/config/raw`.
  If the file is missing, the kernel didn't mount fw_cfg — load the
  `qemu_fw_cfg` module: `modprobe qemu_fw_cfg`.

### Symptom: lamboot-monitor.py errors with "qemu-nbd: failed to attach"

- Storage backend doesn't support nbd export from the live VM. Either
  stop the VM and re-run the monitor against the stopped image, or
  accept that this VM's pre-start monitor invocation will fail silently
  (the hookscript catches monitor failures and logs them as `Failed to
  capture boot health (monitor returned error)`).
- Some NFS-backed storages don't expose efidisk0 in a form `qemu-nbd`
  can read. Filed as a known limitation in §12.

### Symptom: fleet.jsonl grows without bound

Yes, that's the current behavior. Rotation is operator's responsibility.
Drop-in `/etc/logrotate.d/lamboot`:

```
/var/log/lamboot/*.log /var/log/lamboot/*.jsonl {
    weekly
    rotate 8
    compress
    delaycompress
    missingok
    notifempty
    copytruncate
}
```

---

## 12. Limitations / known issues

1. **NFS-storage efidisk reads via qemu-nbd are flaky.** Monitor invocations
   for VMs whose efidisk0 is on NFS may fail silently. Filed; tracked
   in `lamboot-tools-dev` SPEC-LAMBOOT-PVE-SETUP.

2. **post-start phase is a no-op.** Reserved for future use; documented
   so anyone adding a post-start handler can do so without surprises.

3. **No log rotation.** Operator must drop a logrotate snippet (§11).

4. **fleet.toml has no JSON Schema or formal validation.** Misshapen
   TOML silently degrades to "empty config" — no startup-time check tells
   the operator they have a typo. Future work in SDS-2.

5. **No HA-cluster awareness.** Each Proxmox node reads its own
   fleet.toml. If you live-migrate a VM between nodes, the target node's
   fleet.toml determines the role written to its JSON file — there's no
   cluster-wide single source of truth. Acceptable for single-node and
   small-fleet operators; documented limitation for cluster operators.

6. **The hookscript and monitor are version-pinned to 0.8.4 and current,
   respectively.** They are NOT auto-updated when you `apt upgrade
   lamboot` on the host — they live under `/var/lib/vz/snippets/` and
   `/usr/local/bin/`, both outside dpkg's tracked paths by design (so
   operator-customized variants survive package upgrades). Re-install
   manually after upgrading the source files.

7. **No fleet-wide aggregation.** `fleet.jsonl` is per-node. Aggregating
   across nodes is a future-work item; current operators do it via
   log-shipping into Loki/Vector or equivalent.

8. **fw_cfg blob is plaintext.** Anything in `/var/lib/lamboot/<N>.json`
   is readable by any process inside the guest that can open
   `/sys/firmware/qemu_fw_cfg/by_name/opt/lamboot/config/raw` (effectively
   any privileged process). Do not put secrets in fleet.toml.

---

## 13. Validation procedure (end-to-end sanity check)

After install, this is the minimal sequence to confirm everything works:

```bash
# Pick a LamBoot guest VMID (e.g. 364).

# 1. Manually invoke pre-start (idempotent).
/var/lib/vz/snippets/lamboot-hookscript.pl 364 pre-start

# 2. Inspect the JSON the guest will see.
cat /var/lib/lamboot/364.json | jq
# Expect: schema_version=v1, vmid=364, fleet_id, role all populated.

# 3. Confirm the hookscript wrote audit log.
tail -3 /var/log/lamboot/hookscript.log
# Expect: lines for VM 364 (pre-start) including "Refreshed ...".

# 4. Cold-start the VM so it picks up the -fw_cfg arg.
qm stop 364 && qm start 364

# 5. After VM is booted, SSH in as a non-root user and read the blob.
ssh <user>@<vm-ip> cat /sys/firmware/qemu_fw_cfg/by_name/opt/lamboot/config/raw | jq
# Expect: the same JSON the host wrote. If the file path doesn't exist,
# `modprobe qemu_fw_cfg` first, then re-cat.

# 6. Stop the VM and confirm post-stop captured boot health.
qm stop 364
sleep 5
tail -1 /var/log/lamboot/fleet.jsonl | jq
# Expect: a record for vmid=364, status=healthy (or other), with the
# LamBoot state/crash_count/last_entry/timestamp fields populated.
```

If any of these don't match expectations, see §11.

---

## 14. Future expansion (scope-locked references)

This document covers what works **today**. Larger, planned expansions
that would change the surface described here:

- **SDS-2 (`lamboot-pve-setup` proper tool):** automates §4.4 across an
  entire fleet, handles TOML editing safely, provides a `lamboot-pve-setup
  status` command. Not shipped yet; scheduled later per operator decision.
- **Cluster-wide fleet.toml aggregation:** replace per-node fleet.toml
  with a Proxmox cluster filesystem (`/etc/pve/lamboot/fleet.toml`)
  source of truth. Designed but not implemented; would obsolete §12 item 5.
- **Fleet-wide rolling boot-health aggregation:** Loki/Vector recipes,
  or a dedicated `lamboot-fleet-status` aggregator. Tracked in
  `proxmox-host-install/PVE2-HANDOVER-2026-05-27.md`.

Document changes to the schema here will accompany schema bumps in the
hookscript (from v1 to v2 etc.); old fleet.toml files at v1 will continue
to be parsed by future hookscripts via the explicit `schema_version`
check.
