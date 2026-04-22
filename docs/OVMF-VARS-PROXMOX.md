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
- `docs/SB-RECOVERY.md` — recovery procedures
- `docs/KEY-GENERATION.md` — how LamBoot's signing keys are created
- `tools/build-ovmf-vars.sh` — the VARS file generator
