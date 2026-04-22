# MOK Enrollment Guide

**Audience:** anyone running `lamboot-install --signed` on a Secure-Boot-enabled system that uses the distro shim (Config 3 from `docs/SECURE-BOOT-DEPLOYMENT.md` — Ubuntu, Debian, Fedora, and derivatives).

**Time required:** about two minutes during the next reboot.

**What you need handy:** keyboard access to the physical console (or VM console / VNC), a password you will remember for the next couple of minutes.

---

## 1. What this is and why

The distro's shim bootloader is signed by Microsoft's UEFI CA, so every modern firmware trusts it. Shim then decides whether to trust the next-stage bootloader (LamBoot) by checking its signature against two key stores:

1. the firmware's `db` (which only Microsoft-signed things live in by default), then
2. shim's own **MOK** (Machine Owner Key) store, which *you* can add keys to.

LamBoot's signing cert is not in firmware `db` — we don't have a Microsoft-signed presence there. So we need shim to trust it via MOK. The procedure: stage an import, reboot, and confirm the enrollment in **MokManager** (the blue screen shim launches when there's a pending import).

You see MokManager exactly **once** per key enrolled. After that, shim silently trusts LamBoot binaries signed with that cert forever.

---

## 2. What the install script already did

When you ran `lamboot-install --signed` on a Secure-Boot-enabled system, the script:

1. Copied `db.der` (LamBoot's signing cert) to `/boot/efi/EFI/LamBoot/db.der`
2. Ran `mokutil --import /boot/efi/EFI/LamBoot/db.der`
3. Prompted you for a one-time enrollment password — **remember this password.** You will type it in MokManager in a few moments.
4. Staged the pending import in `MokNew` (a UEFI variable that shim reads at next boot)

If any of that didn't happen, see §6 Troubleshooting.

---

## 3. What happens on the next reboot

Reboot the machine now:

```
sudo reboot
```

The firmware hands off to shim as usual. Shim reads the pending-import state and, instead of booting the OS, **launches MokManager**.

---

## 4. MokManager, screen by screen

MokManager is an EFI text-mode application — keyboard-only, blue background, menu-driven. It looks old-fashioned on purpose; that's shim.

### Screen 1 — 10-second timeout

```
Press any key to perform MOK management
```

**Press any key within 10 seconds.** If you miss this, the machine continues booting normally and the pending import is cancelled (you'll need to `sudo mokutil --import` again and reboot).

### Screen 2 — main menu

```
Shim UEFI key management

  Continue boot
  Enroll MOK
  Enroll key from disk
  Enroll hash from disk

Use the arrow keys to move up and down,
use Enter to select.
```

**Use arrow keys → "Enroll MOK" → press Enter.**

### Screen 3 — view key list

```
View key 0
Continue
```

**Select "View key 0" → Enter** to sanity-check the cert before accepting it.

### Screen 4 — cert details

You'll see the cert's fingerprint and subject. For LamBoot production keys, expect:

```
Subject: C=US, ST=IL, O=Lamco Development, OU=LamBoot,
         CN=LamBoot Release Signing Key 2026,
         emailAddress=office@lamco.io
Issued: Apr 21 02:29:13 2026 GMT
Expires: Apr 20 02:29:13 2029 GMT
Public key: RSA 2048
```

If the subject says anything else (especially "LamBoot Dev"), **do not enroll** — that's a dev key that shouldn't exist in a production release. Abort with Esc and investigate.

**Press any key to return to the previous screen.**

### Screen 5 — confirm enrollment

```
Enroll the key(s)?
  No
  Yes
```

**Select "Yes" → Enter.**

### Screen 6 — password prompt

```
Password:
```

**Type the password you set during `lamboot-install --signed`.** Keystrokes are not echoed. Enter when done.

### Screen 7 — final menu

```
Reboot
Continue
```

**Select "Reboot" → Enter.** The machine restarts and, on this reboot, shim now trusts LamBoot and hands off to it normally.

---

## 5. How to verify enrollment worked

After the second reboot (the one after MokManager), log in and run:

```
sudo mokutil --list-enrolled | grep -A3 LamBoot
```

Expected: subject line with `CN=LamBoot Release Signing Key 2026`. If present, enrollment succeeded.

Also confirm no pending imports remain:

```
sudo mokutil --list-new
```

Should say `MokNew is empty` (or produce no output).

Now try booting LamBoot — set it as first in boot order or select it from the firmware boot menu (usually F12/F11 at POST). It should load normally through the shim chain.

---

## 6. Troubleshooting

### MokManager never appeared

**Symptom:** you rebooted, the machine booted straight into Linux, no blue screen.

**Most likely cause:** the pending import was cancelled or never staged.

**Fix:**
```
sudo mokutil --import /boot/efi/EFI/LamBoot/db.der
# enter a password twice
sudo reboot
```

**Second-most likely:** you missed the 10-second "press any key" window on the first reboot. Same fix.

### MokManager appeared but key was rejected with "Failed to match password"

**Cause:** you typed the wrong password. The password is the one you set during `mokutil --import`, not your user password and not the sudo password.

**Fix:** press Esc to exit MokManager, boot into Linux, then:
```
sudo mokutil --import /boot/efi/EFI/LamBoot/db.der
# set a new password you'll remember
sudo reboot
```

### Boot hangs after selecting LamBoot

**Most likely symptoms:**
- blue shim splash, then black screen forever
- "Failed to open MokManager" briefly then hang

**Likely cause (specific to this release):** shim has a known hang when verifying binaries via RSA 4096 MOK keys (see `docs/analysis/RSA-4096-COMPATIBILITY-ANALYSIS-2026-04-20.md`). LamBoot's `db` key is RSA 2048 precisely to avoid this, but if you see the hang, confirm:

```
openssl x509 -in /boot/efi/EFI/LamBoot/db.der -inform DER -noout -text \
    | grep 'Public-Key:'
```

Expected: `Public-Key: (2048 bit)`. If it says 4096, something went wrong in the release build — report the issue.

### "Invalid signature" or verification error

**Cause:** the bootloader was signed with a different key than the one in MOK.

**Fix:**
```
# Verify what's on the ESP
sbverify --cert /boot/efi/EFI/LamBoot/db.der /boot/efi/EFI/LamBoot/grubx64.efi
```

If verification fails, re-run the install:
```
sudo lamboot-install --signed
sudo reboot
```

### MokManager loops — accepts the password but never completes

**Cause:** rare firmware bug; some older AMI firmware has issues writing large MOK variables.

**Fix:**
```
# Boot back to Linux via firmware fallback (F12)
sudo mokutil --reset       # cancel the pending import
# Then try direct db enrollment (Config 2) instead of MOK (Config 3)
# Requires firmware-setup access; see docs/SECURE-BOOT-DEPLOYMENT.md §5
```

### Need to un-enroll a previously accepted cert

```
sudo mokutil --delete /boot/efi/EFI/LamBoot/db.der
# enter a one-time password
sudo reboot
# MokManager appears again; select "Delete MOK" and confirm
```

### Everything is hopeless — emergency firmware-level recovery

If MokManager is broken on this firmware and you can't get in, your options are:

1. **Physical hardware:** reset to Setup Mode via firmware setup → re-enroll platform keys from scratch
2. **VM:** stop the VM, use `qemu-nbd` to mount the ESP from the host, replace LamBoot with the original bootloader, restart
3. **Last resort for VMs:** reset the NVRAM variable store — `qm set <VMID> --delete efidisk0 && qm set <VMID> --efidisk0 ...` — but this wipes ALL enrolled keys including shim's; only useful if you're starting fresh

See `docs/SECURE-BOOT-DEPLOYMENT.md` §7 for detailed recovery procedures.

---

## 7. Why this UX is so bad

Fair question. The answer is: MokManager runs before any OS is loaded, in 16-bit-ish EFI text mode, with a keyboard and not much else. It has to be bulletproof against malicious OS software (otherwise an attacker could script-enroll keys invisibly), so all user confirmation has to happen in a pre-OS environment that nothing in Linux can script. The blue screen, the password prompt, the arrow-key navigation — all deliberate anti-automation measures.

You only go through it once per key, per machine. Future LamBoot updates signed with the same key are accepted automatically.

---

## 8. Related documentation

- `docs/SECURE-BOOT-DEPLOYMENT.md` — the master deployment guide and decision tree
- `docs/SB-RECOVERY.md` — expanded recovery procedures (TBD)
- `docs/analysis/RSA-4096-COMPATIBILITY-ANALYSIS-2026-04-20.md` — why our `db` cert is RSA 2048
- `docs/KEY-GENERATION.md` — how LamBoot's signing keys were generated
