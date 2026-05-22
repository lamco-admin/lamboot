# OVMF VARS Deployment Guide for Proxmox

**Audience:** Proxmox VE operators deploying LamBoot across VM fleets without touching guests' shim or MOK.
**Config:** §6 Config 4 from `docs/SECURE-BOOT-DEPLOYMENT.md`.
**Outcome:** Secure-Boot-enabled VMs that trust LamBoot directly via firmware `db`, no shim chain, no MokManager dance, no guest interaction.

---

## 1. What this is

`OVMF_VARS_lamboot.fd` is a 540 KB binary — a UEFI variable store in the 4MB OVMF format — with these keys pre-populated in firmware NVRAM:

- **PK** — Microsoft's Platform Key (preserved from stock Debian OVMF)
- **KEK** — Microsoft's + Debian's Key Exchange Keys
- **db** — Microsoft UEFI CA 2011, Microsoft Windows Production PCA 2011, **plus LamBoot's signing cert**

Because LamBoot's cert is in `db`, the firmware validates LamBoot binaries directly. No shim. No MOK. Microsoft keys are retained so Windows guests and distro shims continue to work — this file is safe to use for any guest, not just Linux.

Everything Windows/shim needs to boot is untouched. The only behavioural change from stock OVMF VARS is that LamBoot now boots.

### 1.1 Common pitfall: `--add-mok` alone is not enough

Operators rolling their own VARS file with `virt-fw-vars` (instead of using the pre-built `OVMF_VARS_lamboot.fd`) sometimes enroll LamBoot's cert into the **MOK list** with `--add-mok`, expecting that to be sufficient. It is not, for direct boot:

- **MOK** (Machine Owner Key) is consulted by `shim`, not by the firmware. If LamBoot is launched directly (no shim in the chain), the firmware sees no `db` entry that signs LamBoot, rejects the binary, and falls back to whatever other Boot#### entries exist (typically the distro's shim).
- **db** is consulted by the firmware itself. A cert in `db` makes LamBoot directly bootable under SB with no shim wrapper.

The two paths that work under SB:

| Path | Cert location | Tool flag |
|---|---|---|
| **Config 3** — chained behind shim | MOK | `virt-fw-vars --add-mok` (or `mokutil --import` from inside the guest) |
| **Config 4** — direct, no shim | firmware `db` | `virt-fw-vars --add-db` (this document; `OVMF_VARS_lamboot.fd` does this for you) |

Mixing the two — e.g. enrolling to MOK only and expecting LamBoot to boot directly — silently fails by falling back to a different Boot#### entry. Diagnose with `efibootmgr` post-boot: `BootCurrent` will not match LamBoot's Boot####.

This pitfall was observed during v0.9.1 sprint testing on a Fedora workstation: `virt-fw-vars --add-mok` alone left LamBoot un-bootable directly under SB; adding `--add-db` for the same cert fixed it.

---

## 2. When to use this

Choose Config 4 (this document) over Config 3 (shim + MOK) when:

- You're deploying LamBoot across many VMs and want zero per-guest interaction
- You control the Proxmox host
- You want boot to "just work" with no operator steps inside the guest

Choose Config 3 instead when:

- The VM isn't on a hypervisor you control (bare metal, cloud, etc.)
- You want to validate the same trust path a distro end-user would experience

Both configs require a signed LamBoot binary; the difference is in how the firmware trusts it.

---

## 3. Prerequisites

**On the Proxmox host:**
- Proxmox VE 7.x or 8.x (tested on 8.x)
- Root shell access
- The target VM exists and is shut down
- Target VM uses `bios: ovmf` and has an `efidisk0` (Secure Boot must be configured in advance if you want SB enforcement)

**Files you need on the Proxmox host:**
- `OVMF_VARS_lamboot.fd` from your LamBoot release tarball (or `dist/OVMF_VARS_lamboot.fd` from the dev tree)

Copy it onto the Proxmox node:
```
scp dist/OVMF_VARS_lamboot.fd root@pve:/var/lib/vz/snippets/
```

`/var/lib/vz/snippets/` is a convenient location that exists by default on every Proxmox install; any other directory works.

---

## 4. Identifying the target VM's efidisk

From the Proxmox host:

```
qm config <VMID> | grep -E 'bios|efidisk'
```

Expected output:
```
bios: ovmf
efidisk0: <storage>:<volume>,efitype=4m,pre-enrolled-keys=1,size=1M
```

**Three things to check:**

1. `bios: ovmf` — must be present. If `bios: seabios`, this VM is BIOS-only and Secure Boot doesn't apply; use Config 1 (unsigned install) instead.
2. `efitype=4m` — must be `4m`, not the old 64k format. The `OVMF_VARS_lamboot.fd` file is 4MB format only.
3. `<storage>:<volume>` — tells you which Proxmox storage backend holds the efidisk. This drives §5.

If `efitype=4m` isn't set, the VM was created with an older OVMF format. Recreate the efidisk:
```
qm shutdown <VMID>
qm set <VMID> --delete efidisk0
qm set <VMID> --efidisk0 <storage>:1,efitype=4m,pre-enrolled-keys=0
```

---

## 5. Writing the VARS file — by storage backend

The `efidisk0` is a 528 KB volume stored differently depending on `<storage>`'s backend type. `pvesm status` shows each storage's type.

### 5.1 ZFS-backed storage (`zfspool`)

Efidisk is a ZFS volume (zvol) exposed as a block device at `/dev/zvol/<pool>/vm-<VMID>-disk-N`.

```
qm shutdown <VMID>                                 # make sure VM is off
zfs list -t volume | grep vm-<VMID>-disk           # find the zvol (look for ~528K size)

# Write the VARS file over the zvol. The zvol is already sized for OVMF VARS,
# so dd truncates our 540K input safely if needed.
dd if=/var/lib/vz/snippets/OVMF_VARS_lamboot.fd \
   of=/dev/zvol/<pool>/vm-<VMID>-disk-N \
   bs=1M conv=notrunc status=progress
sync

qm start <VMID>
```

### 5.2 LVM / LVM-thin storage (`lvm`, `lvmthin`)

Efidisk is a logical volume at `/dev/<vg>/vm-<VMID>-disk-N`.

```
qm shutdown <VMID>
lvs | grep vm-<VMID>-disk                          # find the efidisk LV

dd if=/var/lib/vz/snippets/OVMF_VARS_lamboot.fd \
   of=/dev/<vg>/vm-<VMID>-disk-N \
   bs=1M conv=notrunc status=progress
sync

qm start <VMID>
```

### 5.3 Directory storage (`dir`)

Efidisk is a raw file at `/var/lib/vz/images/<VMID>/vm-<VMID>-disk-N.raw`.

```
qm shutdown <VMID>
ls -la /var/lib/vz/images/<VMID>/                  # confirm the file exists

cp /var/lib/vz/snippets/OVMF_VARS_lamboot.fd \
   /var/lib/vz/images/<VMID>/vm-<VMID>-disk-N.raw
sync

qm start <VMID>
```

### 5.4 Ceph RBD storage (`rbd`)

Efidisk is an RBD image in a Ceph pool.

```
qm shutdown <VMID>
rbd -p <pool> ls | grep vm-<VMID>-disk             # find the efidisk image

rbd import --image-format 2 --dest-pool <pool> \
    /var/lib/vz/snippets/OVMF_VARS_lamboot.fd \
    vm-<VMID>-disk-efi-tmp

# Replace in-place: delete the old image (after confirming), rename new one
rbd -p <pool> rm vm-<VMID>-disk-N
rbd -p <pool> mv vm-<VMID>-disk-efi-tmp vm-<VMID>-disk-N

qm start <VMID>
```

(Ceph operators may prefer `rbd import --image-format 2` followed by an update to `/etc/pve/qemu-server/<VMID>.conf` to reference the new image name — use whichever workflow fits your backup cadence.)

### 5.5 Other backends

For storage types not listed (iSCSI, NFS, ZFS-over-iSCSI, etc.), the general pattern is:

1. `qm shutdown <VMID>`
2. Determine how `vm-<VMID>-disk-N` is exposed on the host filesystem
3. Write `OVMF_VARS_lamboot.fd` byte-for-byte over that backing storage with `dd`, `cp`, or the backend-native tool
4. `qm start <VMID>`

The Proxmox wiki's [Storage: Raw Files](https://pve.proxmox.com/wiki/Storage) page lists how each backend names and exposes volumes.

---

## 5a. Alternative: in-place modification (preserve existing VARS state)

§5 replaces the entire `efidisk0` with the pre-built `OVMF_VARS_lamboot.fd` template. That is the right approach for **fleet deployment** where every VM should have identical canonical VARS.

For an **already-running individual VM** where you want to keep its existing UEFI state — `BootOrder`, existing `Boot####` entries (Debian's `\EFI\debian\shimx64.efi`, etc.), enrolled MOK entries (DKMS module signing keys, NVIDIA Module Signing), customized PK/KEK — use **in-place modification** instead: read the existing VARS off the efidisk, append LamBoot's cert to `db` with `virt-fw-vars --inplace --add-db`, write it back.

### 5a.1 When to use this path vs §5

| | §5 (template replace) | §5a (in-place modify) |
|---|---|---|
| Fleet deployment from scratch | ✓ | |
| Preserves per-VM `BootOrder`, `Boot####` entries | | ✓ |
| Preserves enrolled MOK (DKMS, NVIDIA module signing, …) | | ✓ |
| Preserves customized PK/KEK (e.g., Debian's PK/KEK) | | ✓ |
| Requires `virt-firmware` on Proxmox host | (only if generating template) | ✓ (always) |
| Snapshot/rollback safety | tied to template integrity | per-modification ZFS snapshot |
| Idempotent (re-running is a no-op if cert already in db) | byte-identical writes | yes, with verification helper |

Rule of thumb: §5 if you have a fleet template you trust; §5a if the VM has been running for a while and accumulated state worth keeping.

### 5a.2 The procedure (ZFS-backed example)

```bash
VMID=108
ZFS_DS="MonsterStore/vm-${VMID}-disk-1"          # adjust to your pool/disk
ZVOL="/dev/zvol/${ZFS_DS}"
CERT="/path/to/keys/db.der"                       # LamBoot signing cert
GUID="4c414d42-4f4f-5400-0000-000000000001"       # LamBoot vendor GUID

qm shutdown "$VMID"

# 1. ZFS snapshot for rollback safety (instant on ZFS)
zfs snapshot "${ZFS_DS}@pre-lamboot-inject-$(date +%Y%m%d-%H%M%S)"

# 2. Read existing VARS off the zvol
SIZE=$(blockdev --getsize64 "$ZVOL")              # 4194304 for 4M efidisk
COUNT_MB=$(( SIZE / 1048576 ))
dd if="$ZVOL" of=/tmp/vars.bak bs=1M count="$COUNT_MB" status=none
cp /tmp/vars.bak /tmp/vars.new

# 3. Append LamBoot cert to db (preserves all existing variables)
virt-fw-vars --inplace /tmp/vars.new --add-db "$GUID" "$CERT"

# 4. VERIFY before writing back (see §5a.3)
# ...

# 5. Write modified VARS back to the zvol
dd if=/tmp/vars.new of="$ZVOL" bs=1M count="$COUNT_MB" conv=notrunc status=none
sync

qm start "$VMID"
```

Rollback: `zfs rollback ${ZFS_DS}@pre-lamboot-inject-<timestamp>` while VM is shut down.

For non-ZFS storage backends, replace step 1's `zfs snapshot` with the equivalent (LVM snapshot, `cp` for directory storage, RBD snapshot, etc.) and steps 2/5's dd with the appropriate read/write mechanism for that backend.

### 5a.3 Verification — `virt-fw-vars --print` does NOT show cert subjects

A common verification mistake: piping `virt-fw-vars --print` through `grep "LamBoot Release Signing Key"` after `--add-db`. **This will fail** even when the cert is correctly added, because `--print` outputs structural information (variable names, blob sizes) but never decodes X.509 cert subjects:

```
db                  : blob: 4225 bytes      # ← size only, no subject string
```

The right verification is `--extract-certs` followed by `openssl x509`:

```bash
verify_lamboot_cert_in_db() {
    local vars_file="$1"
    local expected_sha256="$2"     # e.g. "513a22b6f16a5a13aeebb8da1bfb3e96..."
    local guid="$3"                # e.g. "4c414d42-4f4f-5400-0000-000000000001"

    local tmpdir
    tmpdir=$(mktemp -d /tmp/lamboot-verify-XXXXXX)

    ( cd "$tmpdir" && virt-fw-vars --input "$vars_file" --extract-certs >/dev/null 2>&1 ) \
        || { rm -rf "$tmpdir"; return 1; }

    local cert_file
    cert_file=$(find "$tmpdir" -maxdepth 1 -name "db-${guid}-*.pem" -print -quit)
    [[ -f "$cert_file" ]] || { rm -rf "$tmpdir"; return 1; }

    local fp
    fp=$(openssl x509 -in "$cert_file" -noout -fingerprint -sha256 \
         | sed 's/^.*Fingerprint=//' | tr -d ':' | tr '[:upper:]' '[:lower:]')

    rm -rf "$tmpdir"
    [[ "$fp" == "$expected_sha256" ]]
}
```

This asserts on bytes, not human-readable substrings — robust against virt-fw-vars output format changes and against subject-string typos.

**A secondary sanity check** is to verify the `db` blob grew by the expected amount:

```bash
virt-fw-vars --input /tmp/vars.bak --print | grep '^db '
# db                  : blob: 3143 bytes
virt-fw-vars --input /tmp/vars.new --print | grep '^db '
# db                  : blob: 4225 bytes
```

Delta = 1082 bytes for a typical 1KB DER cert plus the 44-byte EFI_SIGNATURE_LIST + owner GUID overhead. A delta of zero means `--add-db` silently no-op'd; a non-1KB delta means something other than a cert went in.

### 5a.4 1MB efidisk anomaly on older Proxmox VMs

Some VMs created on older Proxmox versions, or restored from older backups, have `efitype=4m` in their config but a **1MB-allocated** efidisk on disk:

```
efidisk0: <storage>:vm-N-disk-X,efitype=4m,pre-enrolled-keys=1,size=1M
```

The OVMF VARS structure (~540KB) fits in 1MB, so the VM still boots fine — but there's almost no slack for additional variable storage. Adding LamBoot's cert (≈1KB) is comfortable, but adding several certs, or larger BootOrder/MOK growth, can run out of space.

Best practice: expand to 4MB before §5a injection.

```bash
# ZFS-backed example:
qm shutdown "$VMID"
zfs snapshot "${ZFS_DS}@pre-lamboot-expand-$(date +%Y%m%d-%H%M%S)"
zfs set volsize=4M "$ZFS_DS"
udevadm settle
qm set "$VMID" --efidisk0 "<storage>:vm-${VMID}-disk-1,efitype=4m,pre-enrolled-keys=1,size=4M"
# Then proceed with §5a.2.
```

The existing ~540KB of OVMF VARS content stays at the start of the now-4MB zvol; the new 3.5MB is zero-padded. Pre-enrolled keys, `BootOrder`, and existing `Boot####` entries are all preserved.

**Do not use `--fresh-template`-style replacement on a VM you want to preserve.** Replacing the efidisk with a fresh `OVMF_VARS_4M.ms.fd` wipes `BootOrder` and all `Boot####` entries; the VM may no longer boot until you recreate the distro boot entry via `grub-install` from a rescue environment.

### 5a.5 Live data: aibox (Debian 13, ZFS, ext2 /boot) — 2026-05-21

First end-to-end exercise of §5a + Config 3 with `db` pre-enrolled (per `SECURE-BOOT-DEPLOYMENT.md §4.1`). VM 108 on a Proxmox host, ZFS-backed efidisk, Debian 13 trixie guest with `/boot=ext2`.

**Pre-install state:**
- `efidisk0: AB:vm-108-disk-1,efitype=4m,pre-enrolled-keys=1,size=1M` (1MB allocated — §5a.4 anomaly)
- ZFS zvol `MonsterStore/vm-108-disk-1`, blockdev size 1048576
- Existing UEFI state non-trivial: Debian PK (not Microsoft's), MS UEFI CA + MS PCA + Debian's KEK in `db`, MokList already enrolled with `DKMS module signing key` + `NVIDIA Module Signing` (per `--extract-certs` on a backup of the pre-modification VARS)

**Expand step (§5a.4):**
- `zfs set volsize=4M MonsterStore/vm-108-disk-1` → new size 4194304
- `qm set 108 --efidisk0 AB:vm-108-disk-1,efitype=4m,pre-enrolled-keys=1,size=4M`
- All existing variables, BootOrder, and MokList preserved (verified by `--extract-certs` before/after)

**Inject step (§5a.2):**
- `dd` zvol → `vars.bak` (4194304 bytes)
- `virt-fw-vars --inplace vars.new --add-db 4c414d42-4f4f-5400-0000-000000000001 db.der`
- `db` blob grew 3143 → 4225 bytes (delta 1082 = expected for 1038-byte DER cert + EFI_SIGNATURE_LIST overhead)
- Verification by `--extract-certs` produced `db-4c414d42-4f4f-5400-0000-000000000001-LamBootReleaseSigningKey2026.pem` with sha256 fingerprint matching expected `51:3A:22:B6:F1:6A:5A:13:…:14:2`
- `dd` vars.new → zvol; round-trip re-read confirms cert on disk

**Install step (`lamboot-install --signed`, in-guest):**
- Auto-detected shim at `/boot/efi/EFI/debian/shimx64.efi`; deployed Config 3 layout (`SECURE-BOOT-DEPLOYMENT.md §4`)
- `/boot/efi/EFI/LamBoot/` contains: `shimx64.efi` (957KB Debian shim), `grubx64.efi` (484KB LamBoot signed binary), `lambootx64.efi` (also LamBoot signed; same sha256), `policy.toml`, `db.der`, `drivers/`, `modules/`, `reports/`
- 5 BLS entries generated for kernels 6.12.{69,73,74+1,85,88}+deb13-amd64
- `efibootmgr --create` made `Boot0001* LamBoot → \EFI\LamBoot\shimx64.efi`; **inserted at head of BootOrder** by default

**Observed first boot:**
- `BootCurrent: 0001` — firmware loaded LamBoot's shim, not Debian's shim
- LamBoot 0.9.0 ran, discovered 5 entries in 1000 ms, selected `bls-debian-6.12.88`
- Kernel `6.12.88+deb13-amd64` booted normally; Debian came up clean
- `boot.json` reports `entry_type: linux_legacy` — kernel loaded via firmware LoadImage, **not** SDS-3 native PE loader

**Open issues surfaced:**

1. **`entry_type: linux_legacy` on Debian 13 / ext2 — expected or regression?**
   The Fedora 122 cross-distro reference (CROSS-DISTRO-TEST-RESULTS-2026-04-25 §1) produced `loader=native_pe_loader backend=ext4-view@0.9.3`. aibox produced `linux_legacy` with no native loader event. Two non-exclusive hypotheses:
   - ext4-view's ext2 support: does ext4-view enumerate ext2 superblocks? If it does, SDS-3 should have engaged. If it doesn't, this is the first ext2-`/boot` data point that should be filed as a v0.9.x test row.
   - Debian shim-protocol interaction: Debian's shim may register a different `SecurityArchProtocol` or `ShimLock` shape than Fedora's, causing LamBoot to prefer the legacy path.
   Reproducer: any Debian guest with ext2 `/boot` under SB+shim, install LamBoot, examine `boot.json`. If `linux_legacy` is consistent, the right fix is to extend ext4-view ext2 coverage or extend SDS-3's filesystem matcher.

2. **v0.9.0 summary `boot.json` cannot prove `verified_via=shim_db`.**
   The trust-log richness the cross-distro test results doc shows (per-event `verified_via`, `image_loaded_native`, PCR measurements) is only emitted on the `native_pe` path. On `linux_legacy`, `boot.json` records timing and entry selection but no per-event verification trace. For evidence of the firmware-db trust path on this run, see §`SECURE-BOOT-DEPLOYMENT.md §4.1` "On evidence" — boot success itself is the proof under this binary version.

3. **Both `grubx64.efi` and `lambootx64.efi` are the signed binary.**
   sha256 `2f14750c…` matches `dist/EFI/LamBoot/lambootx64-signed.efi`. The unsigned `dist/EFI/LamBoot/lambootx64.efi` (sha256 `523f1249…`) was NOT deployed. Worth confirming this is the intended install-script behavior; if so, the spec wording for the unsigned-binary slot (`SPEC-LAMBOOT-INSTALL.md §1.2`) may be stale.

4. **`lamboot_version=0.9.0` in `boot.json` while dev tree `Cargo.toml` is `0.9.1`.**
   The installed binary is from a v0.9.0 build of LamBoot, not freshly rebuilt against v0.9.1 source. This is expected when using `dist/` artifacts, but worth flagging — anyone running this procedure to validate v0.9.1 behavior needs to rebuild `dist/` first (`./build.sh`).

5. **Debian PK observed in pre-existing VARS.**
   Stock Proxmox `pre-enrolled-keys=1` is documented to enroll Microsoft's PK. aibox had Debian's PK already, suggesting prior `mokutil`/`update-secureboot-policy` activity in this guest. Not a v0.9.x concern but worth noting in §5a.1's "preserve per-VM state" rationale.

**Confirmed working: §5a + §4.1 hybrid path on Debian 13 + SB + ZFS efidisk + ext2 /boot.** Doesn't fill `P-TM-1` (which wants `verified_via=shim_mok`, a different code path), but populates a previously-blank cell in the test matrix.

---

## 6. After swapping VARS

Boot the VM. From the guest:

```
mokutil --sb-state
# expected: SecureBoot enabled

# LamBoot's cert should now appear in firmware db:
sudo apt install efitools  # or equivalent
sudo efi-readvar -v db | grep -A2 'LamBoot'
# expected: Subject: C=US, ST=IL, O=Lamco Development, OU=LamBoot,
#           CN=LamBoot Release Signing Key 2026
```

Now install LamBoot with the direct-boot path (no shim, no MOK):

```
sudo lamboot-install --signed --no-shim
```

Reboot. LamBoot loads directly — firmware validates its signature against the `db` entry you installed, hands off, LamBoot splash appears.

---

## 7. Regenerating `OVMF_VARS_lamboot.fd` with production keys

The release tarball ships `OVMF_VARS_lamboot.fd` pre-built. If you need to rebuild it (fleet key rotation, test builds, custom cert composition), use `tools/build-ovmf-vars.sh`:

```
# Requires: pip install virt-firmware (or a venv with it)
# Requires: /usr/share/OVMF/OVMF_VARS_4M.ms.fd from the ovmf package

cd ~/lamboot-dev
./tools/build-ovmf-vars.sh --cert keys/db.crt --output dist/OVMF_VARS_lamboot.fd
```

The script takes the stock Microsoft-enrolled Debian OVMF VARS template and appends LamBoot's cert to the `db` variable. Microsoft keys are preserved.

If you are scripting around `virt-fw-vars` directly instead of using `build-ovmf-vars.sh`, use `--add-db` for direct LamBoot boot under SB. `--add-mok` alone is not sufficient for direct boot; see §1.1 for why.

**Note on key rotation:** when the LamBoot `db` key rotates (planned 2029), `OVMF_VARS_lamboot.fd` must be regenerated and re-deployed to every Config 4 VM. Plan for a maintenance window or roll out alongside existing update workflows.

---

## 8. Rollback

If LamBoot fails to boot and you need the VM back on its original bootloader:

```
qm shutdown <VMID>

# Restore stock Proxmox OVMF VARS (Microsoft-only db):
qm set <VMID> --delete efidisk0
qm set <VMID> --efidisk0 <storage>:1,efitype=4m,pre-enrolled-keys=1

qm start <VMID>
```

Stock Proxmox OVMF VARS has Microsoft keys, no LamBoot cert, so Windows/shim-based guests continue to boot normally. LamBoot binaries will be rejected — the `\EFI\LamBoot\` tree can then be cleaned up from inside the guest with `lamboot-install --remove`.

---

## 9. Fleet automation

For large deployments, wrap §5 in a helper script:

```bash
#!/bin/bash
# deploy-lamboot-vars.sh — swap a VM's efidisk to OVMF_VARS_lamboot.fd
set -e
VMID="$1"
VARS="/var/lib/vz/snippets/OVMF_VARS_lamboot.fd"
[ -f "$VARS" ] || { echo "$VARS not found"; exit 1; }
[ -n "$VMID" ] || { echo "Usage: $0 <VMID>"; exit 1; }

qm shutdown "$VMID"

# Locate and identify efidisk storage — adapt to your backend
EFIDISK=$(qm config "$VMID" | awk -F: '/^efidisk0:/ {print $2}' | cut -d, -f1)
# ... (add per-backend write logic here; pattern from §5)

qm start "$VMID"
```

Iterate across a VMID list to deploy the fleet. Expect each VM's efidisk swap to take a few seconds.

---

## 10. libvirt / virt-manager (brief)

The same `OVMF_VARS_lamboot.fd` works for libvirt. Edit the domain XML:

```xml
<os firmware='efi'>
  <nvram template='/usr/share/OVMF/OVMF_VARS_4M.ms.fd'>/var/lib/libvirt/qemu/nvram/<domain>_VARS.fd</nvram>
  <firmware>
    <feature enabled='yes' name='secure-boot'/>
    <feature enabled='yes' name='enrolled-keys'/>
  </firmware>
</os>
```

Replace the `<nvram>` target with `OVMF_VARS_lamboot.fd`:

```
virsh destroy <domain>
cp /path/to/OVMF_VARS_lamboot.fd /var/lib/libvirt/qemu/nvram/<domain>_VARS.fd
virsh start <domain>
```

Full libvirt coverage: out of scope for this document.

---

## 11. Related documentation

- `docs/SECURE-BOOT-DEPLOYMENT.md` — master guide and config decision tree
- `docs/MOK-ENROLLMENT-GUIDE.md` — Config 3 alternative (guest-side enrollment)
- `docs/KEY-GENERATION.md` — how LamBoot's signing keys are created
- `tools/build-ovmf-vars.sh` — the VARS file generator (canonical source)

---

## 12. Operator tooling — `lamboot-pve-ovmf-vars`

For Proxmox host operators who prefer a subcommand-style UX over invoking
`build-ovmf-vars.sh` directly, the companion toolkit ships
`lamboot-pve-ovmf-vars` in the `lamboot-toolkit-pve` RPM subpackage (see
[`LAMBOOT-TOOLS-OVERVIEW.md`](LAMBOOT-TOOLS-OVERVIEW.md)).

`lamboot-pve-ovmf-vars` is **a mirror** of `tools/build-ovmf-vars.sh` in
this repo — canonical source stays here; the mirror is regenerated at
toolkit release-build time. Do not edit the mirror directly; edit
`tools/build-ovmf-vars.sh` here, then re-run the toolkit's mirror script.

Subcommands (from the mirror):

- `build` — build `OVMF_VARS_lamboot.fd` with LamBoot's db cert pre-enrolled
- `verify` — inspect an existing VARS file for the expected enrollment
- `show` — print enrolled keys in human-readable form

The tool exists for discoverability in the broader `lamboot-toolkit-pve`
suite. Operators already running `tools/build-ovmf-vars.sh` from this
repo's release tarball need not change anything.

Tool-level documentation lives with the [`lamboot-tools`](https://github.com/lamco-admin/lamboot-tools)
repository (OVMF-VARS helper is a sub-component of the PVE setup tooling).
