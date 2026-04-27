# LamBoot Quick Start

**Version:** 0.8.3 — Updated 2026-04-21

## From a release tarball (recommended)

```bash
# Download
curl -L -O https://github.com/lamco-admin/lamboot/releases/download/v0.8.3/lamboot-0.8.3-x86_64.tar.gz
sha256sum -c lamboot-0.8.3-x86_64.tar.gz.sha256

tar xzf lamboot-0.8.3-x86_64.tar.gz && cd lamboot-0.8.3

# Install (Secure Boot off — homelab / dev)
sudo ./lamboot-install

# Install with Secure Boot + MOK enrollment (Linux desktop)
sudo ./lamboot-install --signed
# → reboot → MokManager → Enroll MOK → passphrase → Yes → Reboot
```

## From source

```bash
# Install toolchain
rustup install nightly
rustup target add x86_64-unknown-uefi aarch64-unknown-uefi

# Build
./build.sh

# Output: dist/EFI/LamBoot/lambootx64.efi (~215 KB, x86_64)
#         dist/EFI/LamBoot/lambootaa64.efi (aarch64)
```

## Test in QEMU

Prerequisites: `qemu-system-x86_64`, OVMF firmware.

```bash
# Debian/Ubuntu
sudo apt install qemu-system-x86 ovmf

# Fedora
sudo dnf install qemu-system-x86 edk2-ovmf

# Run
./run-qemu.sh
```

You should see the graphical boot menu with a mouse cursor.

## Manual install on a real system

```bash
# Mount ESP
sudo mount /dev/sdXN /boot/efi

# Copy LamBoot
sudo mkdir -p /boot/efi/EFI/LamBoot/{drivers,modules,reports}
sudo cp -a dist/EFI/LamBoot/* /boot/efi/EFI/LamBoot/

# Write policy
sudo cp examples/policy.toml /boot/efi/EFI/LamBoot/

# Create UEFI boot entry
sudo efibootmgr -c -d /dev/sdX -p N \
    -l '\EFI\LamBoot\lambootx64.efi' -L 'LamBoot'
```

Prefer `sudo ./lamboot-install` — it handles distro detection, shim chain wiring, BLS entry generation, and kernel-install plugin deployment automatically.

## Create BLS entries

If you're not using `kernel-install`, create entries manually under `/boot/efi/loader/entries/`:

```ini
# /boot/efi/loader/entries/linux-6.12.0.conf
title      My Linux (6.12.0)
version    6.12.0
linux      /vmlinuz-6.12.0
initrd     /initramfs-6.12.0.img
options    root=UUID=your-uuid ro quiet
```

Or install the kernel-install plugin for automatic entry creation:

```bash
sudo cp dist/kernel-install/90-lamboot.install /usr/lib/kernel/install.d/
```

Debian/Ubuntu also get kernel hooks automatically via `lamboot-install`:
`/etc/kernel/postinst.d/zz-lamboot` regenerates BLS entries after `apt install linux-image-*`.

## Keyboard and mouse controls

| Input | Action |
|---|---|
| Up / Down arrows | Navigate entries |
| Enter | Boot selected entry |
| Mouse click | Select and boot entry |
| F1 | Help screen |
| F2 | Reboot to firmware setup (UEFI BIOS menu) |
| F12 | Cold reboot |
| Esc | Reset auto-boot timeout / back |

## Next steps

- [**README.md**](README.md) — feature overview
- [**docs/USER-GUIDE.md**](docs/USER-GUIDE.md) — full user documentation
- [**docs/SECURE-BOOT-DEPLOYMENT.md**](docs/SECURE-BOOT-DEPLOYMENT.md) — four Secure Boot configurations
- [**docs/SECURITY-MODEL.md**](docs/SECURITY-MODEL.md) — honest threat model
- [**docs/PROXMOX-GUIDE.md**](docs/PROXMOX-GUIDE.md) — VM fleet deployment
- [**docs/TROUBLESHOOTING-GUIDE.md**](docs/TROUBLESHOOTING-GUIDE.md) — common issues
