# LamBoot Proxmox Integration Guide

**Version:** 0.8.3
**Updated:** 2026-04-21

## Overview

LamBoot is designed as a VM-first bootloader for Proxmox/QEMU/KVM environments. Its key advantages over GRUB and systemd-boot in VM fleets:

- **GUI system info display** — VM ID, hypervisor, Secure Boot state visible at boot
- **Host-side boot health monitoring** via NVRAM variables (no guest agent needed)
- **Automatic VMID injection** via hookscript — no manual config per VM
- **Crash loop detection** with automatic fallback and snapshot-aware reset
- **Compact binary** for fast VM cold boot
- **Pre-boot diagnostics** accessible without booting the guest OS
- **Offline repair** from the Proxmox host using lamboot-repair

## Installation in a Proxmox VM

### Prerequisites

- Proxmox VE 7.x or 8.x
- VM configured with OVMF (UEFI) firmware (`bios: ovmf` in VM config)
- EFI disk (`efidisk0`) present

### Step 1: Install LamBoot in the Guest

```bash
# Inside the guest VM:
sudo mount /dev/vda1 /boot/efi  # or wherever your ESP is

# Copy LamBoot files
sudo mkdir -p /boot/efi/EFI/LamBoot/{drivers,modules,reports}
sudo cp lambootx64.efi /boot/efi/EFI/LamBoot/
sudo cp policy.toml /boot/efi/EFI/LamBoot/

# Create UEFI boot entry
sudo efibootmgr -c -d /dev/vda -p 1 \
    -l '\EFI\LamBoot\lambootx64.efi' -L 'LamBoot'

# (Optional) Set LamBoot as first boot option
sudo efibootmgr -o XXXX  # where XXXX is the LamBoot entry number
```

### Step 2: Install Filesystem Drivers (Optional)

If `/boot` is on an ext4 or btrfs partition (common on Fedora, Arch, etc.):

```bash
sudo cp ext4_x64.efi /boot/efi/EFI/LamBoot/drivers/
sudo cp btrfs_x64.efi /boot/efi/EFI/LamBoot/drivers/  # if needed
```

### Step 3: Install kernel-install Plugin (Optional)

For automatic BLS entry creation when kernels are installed:

```bash
sudo cp 90-lamboot.install /usr/lib/kernel/install.d/
sudo chmod +x /usr/lib/kernel/install.d/90-lamboot.install

# Create the entries directory on the ESP
sudo mkdir -p /boot/efi/loader/entries
```

### Step 4: Configure Policy

```bash
sudo cat > /boot/efi/EFI/LamBoot/policy.toml << 'EOF'
version = 1
default_timeout_ms = 3000

[security]
crash_threshold = 3
fallback_order = ["fallback"]

[watchdog]
enabled = true
grace_seconds = 10
EOF
```

## Host-Side Monitoring

### Setup on Proxmox Host

```bash
# Copy the monitor script to the Proxmox host
scp tools/lamboot-monitor.py root@proxmox:/usr/local/bin/

# Make executable
chmod +x /usr/local/bin/lamboot-monitor.py

# Test it
lamboot-monitor.py
```

### Automated Monitoring

Create a systemd timer for periodic checks:

```bash
# /etc/systemd/system/lamboot-monitor.timer
cat > /etc/systemd/system/lamboot-monitor.timer << 'EOF'
[Unit]
Description=LamBoot VM boot health check

[Timer]
OnBootSec=60
OnUnitActiveSec=300

[Install]
WantedBy=timers.target
EOF

# /etc/systemd/system/lamboot-monitor.service
cat > /etc/systemd/system/lamboot-monitor.service << 'EOF'
[Unit]
Description=LamBoot VM boot health monitor

[Service]
Type=oneshot
ExecStart=/usr/local/bin/lamboot-monitor.py --alert-webhook http://your-webhook-url
EOF

systemctl daemon-reload
systemctl enable --now lamboot-monitor.timer
```

### Webhook Integration

The monitor sends JSON alerts to any webhook endpoint:

```json
{
  "alert": "lamboot-crash-loop",
  "timestamp": "2026-03-27T12:00:00",
  "vms": [
    {
      "vmid": 102,
      "name": "broken-vm",
      "state": "CrashLoop",
      "crash_count": 5,
      "last_entry": "fedora-6.12.0",
      "status": "critical"
    }
  ]
}
```

Compatible with: Slack incoming webhooks, PagerDuty, Grafana OnCall, ntfy.sh, custom HTTP endpoints.

## VM Identity and Fleet Management

### Displaying VMID on the Boot Screen

LamBoot can display the Proxmox VMID and fleet tags on the boot screen and in boot reports. This is configured via SMBIOS OEM string injection in the VM config.

**Step 1: Add SMBIOS OEM strings to the VM config**

On the Proxmox host, edit `/etc/pve/qemu-server/<vmid>.conf`:

```
args: -smbios type=11,value=lamboot.vmid=201
```

Or for fleet management with multiple tags:

```
args: -smbios type=11,value=lamboot.vmid=201,value=lamboot.fleet-id=prod-cluster-01,value=lamboot.role=webserver
```

**Step 2: Verify in LamBoot**

After rebooting the VM, LamBoot reads these OEM strings automatically:
- The VMID appears in the boot report (`/EFI/LamBoot/reports/boot.json`)
- The fleet-id is logged at boot and included in the report
- Serial console shows: `SMBIOS OEM strings: N`

**What the boot report includes:**

```json
{
  "lamboot_version": "0.2.0",
  "vmid": "201",
  "fleet_id": "prod-cluster-01",
  "hypervisor": "KVM",
  "system_manufacturer": "QEMU",
  "system_product": "Standard PC (Q35 + ICH9, 2009)",
  "iommu": "Intel VT-d",
  "iommu_units": 2,
  "boot_timing_ms": {"health":5,"drivers":120,"discovery":45,"total":210}
}
```

**Available OEM string keys:**

| Key | Purpose | Example |
|-----|---------|---------|
| `lamboot.vmid` | Proxmox VM ID | `201` |
| `lamboot.fleet-id` | Fleet/cluster identifier | `prod-cluster-01` |
| `lamboot.role` | VM role tag | `webserver` |
| `lamboot.monitor` | Monitoring endpoint URL | `http://10.0.0.1:9090` |

**For VM templates:** Add the `args:` line to the template config. Override per-clone by editing the cloned VM's config with the correct VMID.

### Hypervisor Detection

LamBoot automatically detects the hypervisor via CPUID:
- **KVM** (Proxmox/QEMU) — detected as "KVM"
- **Hyper-V**, **VMware**, **Xen**, **VirtualBox** — all detected
- Bare metal — "not detected" (graceful)

This appears in the boot report `hypervisor` field and serial console output.

### IOMMU Group Detection

LamBoot parses ACPI DMAR (Intel VT-d) or IVRS (AMD-Vi) tables to detect IOMMU hardware units. This is valuable for PCI passthrough troubleshooting — you can see IOMMU group assignments before Linux boots.

The boot report includes `iommu` (type) and `iommu_units` (count of DRHD/IVHD blocks).

## VM Template Workflow

### Creating a LamBoot-Ready Template

1. Create a new VM in Proxmox with OVMF firmware
2. Install your base OS (Fedora, Ubuntu, Debian, etc.)
3. Install LamBoot (steps above)
4. Install the kernel-install plugin
5. Test: reboot and verify LamBoot shows the boot menu
6. Convert to template in Proxmox

### Cloning from Template

VMs cloned from the template automatically get LamBoot. Each clone:
- Gets a new OVMF_VARS.fd (fresh NVRAM, state=Fresh)
- Has LamBoot on the ESP
- Will auto-detect the installed OS via BLS entries

## How NVRAM Monitoring Works

```
[Guest VM]                              [Proxmox Host]
LamBoot writes UEFI NVRAM vars   ->   OVMF stores to efidisk0
  - LamBootState                        (qcow2 or raw image)
  - LamBootCrashCount                          |
  - LamBootLastEntry                           v
  - LamBootTimestamp              lamboot-monitor.py reads the
                                  efidisk image file directly
                                  (no qemu-nbd needed for raw
                                   OVMF_VARS.fd files)
                                           |
                                           v
                                  Searches for LAMBOOT vendor
                                  GUID bytes in the variable
                                  store, extracts values
                                           |
                                           v
                                  Assesses health:
                                    OK = BootedOK, count=0
                                    WARN = Booting (recent)
                                    CRIT = CrashLoop or high count
                                           |
                                           v
                                  Alert via webhook / JSON / table
```

No guest agent is needed. The monitoring is completely non-intrusive.

## Hookscript — Automated VMID and Monitoring

The LamBoot hookscript integrates with Proxmox's VM lifecycle to automatically inject VMID into SMBIOS OEM strings and capture boot health data.

### Installation

```bash
# On the Proxmox host:
cp lamboot-hookscript.pl /var/lib/vz/snippets/
chmod +x /var/lib/vz/snippets/lamboot-hookscript.pl

# Enable for a VM:
qm set 201 --hookscript local:snippets/lamboot-hookscript.pl
```

### What It Does

| Phase | Action |
|-------|--------|
| **pre-start** | Auto-injects `lamboot.vmid=VMID` into the VM's SMBIOS OEM strings (if not already set). Checks previous boot health and logs warnings for crash loop state. |
| **post-start** | Logs VM start event. |
| **post-stop** | Captures boot health from OVMF NVRAM to fleet log. Calls `lamboot-monitor.py` for health assessment. |

### Automatic VMID Injection

On every VM start, the hookscript checks if `lamboot.vmid=` is in the VM's `args:` config line. If not, it adds:

```
args: -smbios type=11,value=lamboot.vmid=201
```

This means:
- **No manual `qm set --args` needed** — the hookscript handles it
- **Works with cloned VMs** — each clone gets its correct VMID automatically
- **Non-destructive** — existing args are preserved, VMID is appended

### Fleet Log

The hookscript appends health assessments to `/var/log/lamboot/fleet.jsonl` (one JSON record per line). This provides a historical record of boot health across all VMs with the hookscript enabled.

## GUI System Information

When running in a Proxmox VM with VMID configured, the LamBoot boot menu header shows:

- **VM ID** (large text, top right): e.g., `VM 201`
- **Hypervisor**: Auto-detected via CPUID (shows `KVM` for Proxmox)
- **Build info**: Secure Boot state, filesystem driver count

This information is gathered automatically — the VMID from SMBIOS OEM strings (injected by the hookscript), the hypervisor from CPUID, and Secure Boot from UEFI variables.

## Deployment Options

### Option A: Guest-Side Install (Recommended)

Use `lamboot-install` inside the guest VM:

```bash
# SSH into the VM
ssh root@vm-hostname

# Run the installer
sudo lamboot-install --with-modules
```

This is the standard approach — the installer handles ESP detection, driver selection, BLS entry generation, and systemd integration automatically.

### Option B: Host-Side Deploy via qemu-nbd

For VMs that can't boot or for batch deployment:

```bash
# On the Proxmox host:
qm stop VMID
lvchange -ay pve/vm-VMID-disk-N
qemu-nbd --connect=/dev/nbd0 -f raw /dev/pve/vm-VMID-disk-N
mount /dev/nbd0p1 /tmp/esp

# Copy LamBoot files
cp -r dist/EFI/LamBoot /tmp/esp/EFI/

umount /tmp/esp
qemu-nbd -d /dev/nbd0
qm start VMID
```

This method bypasses the installer — no BLS entry generation, no systemd integration. Suitable for deploying updated binaries to existing installations.

### Option C: Offline Repair

If a VM won't boot, use `lamboot-repair` from the host:

```bash
sudo lamboot-repair --offline /dev/pve/vm-VMID-disk-N
```

See [LamBoot Tools](LAMBOOT-TOOLS-OVERVIEW.md) for details.

## Troubleshooting

### LamBoot doesn't appear in boot menu

```bash
# Check if UEFI entry exists
efibootmgr -v

# Re-create entry
efibootmgr -c -d /dev/vda -p 1 -l '\EFI\LamBoot\lambootx64.efi' -L 'LamBoot'
```

### BLS entries not found

```bash
# Verify entries exist
ls /boot/efi/loader/entries/

# If /boot is on ext4, install the ext4 driver
cp ext4_x64.efi /boot/efi/EFI/LamBoot/drivers/
```

### VM stuck in CrashLoop

On the Proxmox host:
```bash
# Check status
lamboot-monitor.py --vmid 102

# To reset: stop VM, clear NVRAM by removing and re-adding efidisk0
# Or: boot from rescue media and clear the NVRAM variable
```

### Monitor can't read OVMF_VARS

```bash
# Check permissions
ls -la /var/lib/vz/images/100/

# pvesm path may need root access
sudo lamboot-monitor.py --vmid 100
```

---

## See Also

- [User Guide](USER-GUIDE.md) — general LamBoot usage
- [Configuration Guide](CONFIGURATION-GUIDE.md) — policy.toml, SMBIOS OEM strings
- [Troubleshooting Guide](TROUBLESHOOTING-GUIDE.md) — comprehensive problem solving
- [LamBoot Tools](LAMBOOT-TOOLS-OVERVIEW.md) — diagnostic and repair toolkit
- [Security Guide](SECURITY-GUIDE.md) — TPM, Secure Boot, crash loop details
