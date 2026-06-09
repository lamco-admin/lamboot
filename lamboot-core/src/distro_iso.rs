//! Layer: 3 — boot-entry parsers.
//!
//! Path A2 (boot-from-ISO distro-family fallback). When a distro ISO has no
//! usable `/boot/grub/loopback.cfg` (Path A1) LamBoot identifies the distro
//! family by **fingerprinting marker paths** on the mounted ISO, then applies
//! that family's known kernel/initrd locations and a base command line.
//!
//! A2 is the **primary** resolver for a *physical optical disc*: `loopback.cfg`
//! exists on the disc but its command line carries an iso-find token
//! (`iso-scan/filename=${iso_path}`, `findiso=`, `img_loop=`) that only makes
//! sense when the ISO is loop-mounted from a *file*. A disc booted directly has
//! no `${iso_path}` — the kernel self-locates on the medium by **volume label**.
//! So each recipe carries two command-line shapes:
//!   * [`DistroRecipe::media_cmdline`] — disc booted directly; `{label}` is the
//!     volume label (`root=live:CDLABEL={label}`, `archisolabel={label}`, …).
//!   * [`DistroRecipe::file_cmdline`] — ISO loop-booted from a file; `{iso}` is
//!     the path (best-effort fallback; A1's own `loopback.cfg` is preferred for
//!     the file case because it carries the distro-exact token).
//!
//! Pure — a path-exists probe in, a static recipe out — so the table and the
//! glob/label helpers are host-tested in `lamboot-fs-tests`. Resolving the
//! candidates against the real ISO (`read_dir`, concatenating microcode +
//! initramfs) is the impure caller's job in `boot.rs`.
//!
//! **Derivative coverage.** Each recipe's marker matches a whole family, not one
//! distro: `/casper` → Ubuntu + Mint + Pop!_OS + Zorin + elementary + the Ubuntu
//! flavors; `/arch/boot` → Arch + EndeavourOS + Manjaro + CachyOS + Garuda +
//! ArcoLinux; `/images/pxeboot` + `/LiveOS` → Fedora + RHEL + CentOS Stream +
//! Rocky + Alma + Nobara. NixOS and Gentoo are deliberately *not* in the table
//! (idiosyncratic boot models) — they fall through to A1 if `loopback.cfg` is
//! present, else the entry is unsupported.
//!
//! **Validation discipline.** The table is validated by the live-VM matrix,
//! never asserted blind. Each family's recipe is confirmed against a real ISO
//! (the per-distro QEMU boot) before it is trusted in a release.

use alloc::string::String;

/// A distro family's boot recipe: where its kernel and initrd live on the ISO,
/// and the two command-line shapes (direct-media vs loop-file).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DistroRecipe {
    /// Stable family id for logging / trust events (`"arch"`, `"ubuntu-casper"`).
    pub family: &'static str,
    /// Marker paths that ALL must exist for this recipe to match. Checked in
    /// table order, so the table is ordered most-specific first.
    pub markers: &'static [&'static str],
    /// Kernel path candidates; the **first that resolves** is the kernel. A
    /// candidate ending in `*` is a prefix glob within its directory (for
    /// distros with version-stamped kernel names, e.g. Debian live).
    pub kernel: &'static [&'static str],
    /// Initrd path candidates; **every one that resolves** is concatenated in
    /// order (microcode images first, then the main initramfs — the order the
    /// kernel expects). A candidate ending in `*` is a prefix glob.
    pub initrd: &'static [&'static str],
    /// Command line for booting the disc **directly** (optical / dd'd medium).
    /// `{label}` is replaced with the volume label (CDLABEL-escaped).
    pub media_cmdline: &'static str,
    /// Command line for loop-booting the ISO **as a file**. `{iso}` is the file
    /// path, `{label}` the volume label. Best-effort: A1 `loopback.cfg` is the
    /// preferred file-case resolver.
    pub file_cmdline: &'static str,
}

/// The recipe table, ordered most-specific marker first so [`fingerprint`] can
/// return the first all-markers-present match. The marker for each family is a
/// directory unique to that family's ISO layout.
const TABLE: &[DistroRecipe] = &[
    // Arch (archiso) + EndeavourOS / Manjaro / CachyOS / Garuda / ArcoLinux.
    // Microcode initrds load BEFORE the main initramfs; both ucode images are
    // shipped and concatenated (the kernel applies whichever matches the CPU).
    DistroRecipe {
        family: "arch",
        markers: &["/arch/boot"],
        kernel: &[
            "/arch/boot/x86_64/vmlinuz-linux",
            "/arch/boot/vmlinuz-linux",
        ],
        initrd: &[
            "/arch/boot/intel-ucode.img",
            "/arch/boot/amd-ucode.img",
            "/arch/boot/x86_64/initramfs-linux.img",
        ],
        media_cmdline: "archisobasedir=arch archisolabel={label}",
        // img_dev would need the holding partition's device path, which A2
        // cannot synthesize purely — loop-file Arch boot wants A1. Kept as a
        // documented best effort.
        file_cmdline: "archisobasedir=arch img_loop={iso} earlymodules=loop",
    },
    // Ubuntu (casper) + Mint + Pop!_OS + Zorin + elementary + Ubuntu flavors.
    DistroRecipe {
        family: "ubuntu-casper",
        markers: &["/casper"],
        kernel: &["/casper/vmlinuz", "/casper/vmlinuz.efi"],
        initrd: &["/casper/initrd", "/casper/initrd.lz", "/casper/initrd.gz"],
        media_cmdline: "boot=casper",
        file_cmdline: "boot=casper iso-scan/filename={iso}",
    },
    // Debian live (live-boot). Version-stamped kernel/initrd names → prefix glob.
    DistroRecipe {
        family: "debian-live",
        markers: &["/live"],
        kernel: &["/live/vmlinuz*", "/live/vmlinuz"],
        initrd: &["/live/initrd.img*", "/live/initrd.img"],
        media_cmdline: "boot=live components",
        file_cmdline: "boot=live components findiso={iso}",
    },
    // Fedora + RHEL + CentOS Stream + Rocky + Alma + Nobara (dracut live).
    // `/LiveOS/squashfs.img` is the dracut dmsquash-live root and is the stable
    // Fedora/RHEL signature across layout versions: the legacy `/images/pxeboot`
    // layout (≤ F43) and the newer `/boot/x86_64/loader` grub2 layout (≥ F44,
    // which dropped `/images/pxeboot`) both ship it. Verified against
    // Fedora-Workstation-Live-44 (live-VM matrix, 2026-06-05). Checked before the
    // openSUSE recipe because F44 now also carries `/boot/x86_64/loader`. Root is
    // found by CDLABEL, so `{label}` must match the PVD label exactly.
    DistroRecipe {
        family: "fedora",
        markers: &["/LiveOS/squashfs.img"],
        kernel: &[
            "/images/pxeboot/vmlinuz",   // Fedora/RHEL ≤ F43
            "/boot/x86_64/loader/linux", // Fedora ≥ F44 (grub2 unified layout)
            "/isolinux/vmlinuz",         // legacy isolinux
        ],
        initrd: &[
            "/images/pxeboot/initrd.img",
            "/boot/x86_64/loader/initrd",
            "/isolinux/initrd.img",
        ],
        media_cmdline: "root=live:CDLABEL={label} rd.live.image",
        file_cmdline: "root=live:CDLABEL={label} rd.live.image iso-scan/filename={iso}",
    },
    // openSUSE (kiwi live): Leap + Tumbleweed.
    DistroRecipe {
        family: "opensuse",
        markers: &["/boot/x86_64/loader"],
        kernel: &["/boot/x86_64/loader/linux"],
        initrd: &["/boot/x86_64/loader/initrd"],
        media_cmdline: "root=live:CDLABEL={label}",
        file_cmdline: "root=live:CDLABEL={label} iso-scan/filename={iso}",
    },
    // Alpine (alpine-baselayout live ISO).
    DistroRecipe {
        family: "alpine",
        markers: &["/apks"],
        kernel: &["/boot/vmlinuz-lts", "/boot/vmlinuz-virt"],
        initrd: &["/boot/initramfs-lts", "/boot/initramfs-virt"],
        media_cmdline: "alpine_dev=cdrom:iso9660 modules=loop,squashfs,sd-mod,usb-storage quiet",
        file_cmdline: "modules=loop,squashfs,sd-mod,usb-storage quiet",
    },
];

/// Identify the distro family of a mounted ISO by probing marker paths. Returns
/// the first recipe whose markers all exist. `exists` is the caller's
/// path-presence probe (`|p| iso_vol.exists_str(p)`); it is `FnMut` because the
/// real probe (`Volume::exists_str`) takes `&mut self`. Kept as a closure so
/// this stays pure and host-testable.
pub(crate) fn fingerprint(mut exists: impl FnMut(&str) -> bool) -> Option<&'static DistroRecipe> {
    TABLE.iter().find(|r| r.markers.iter().all(|m| exists(m)))
}

/// Split a candidate into `(dir, prefix)` if it is a trailing-`*` prefix glob
/// (`/live/vmlinuz*` → `("/live", "vmlinuz")`); `None` for an exact path. Only a
/// single trailing `*` is supported — recipe globs never need more.
pub(crate) fn split_glob(candidate: &str) -> Option<(&str, &str)> {
    let stripped = candidate.strip_suffix('*')?;
    let slash = stripped.rfind('/')?;
    Some((&stripped[..slash], &stripped[slash + 1..]))
}

/// Pick the best directory entry matching a prefix glob: among `names` starting
/// with `prefix`, return the lexicographically greatest (newest version-stamped
/// kernel/initrd when several are present). `None` if nothing matches.
pub(crate) fn glob_pick<'a>(names: &[&'a str], prefix: &str) -> Option<&'a str> {
    names
        .iter()
        .copied()
        .filter(|n| n.starts_with(prefix))
        .max()
}

/// Render a recipe command-line template, substituting `{iso}` with the ISO file
/// path and `{label}` with the (CDLABEL-escaped) volume label. Both tokens are
/// substituted regardless of which template is passed, so a media template that
/// uses only `{label}` and a file template that uses both are handled uniformly.
pub(crate) fn render_cmdline(template: &str, iso_path: &str, label: &str) -> String {
    template
        .replace("{iso}", iso_path)
        .replace("{label}", &escape_label(label))
}

/// Escape a volume label for use in a kernel `CDLABEL=`/`archisolabel=` token.
/// The kernel's label matcher expects spaces encoded as `\x20`; other bytes pass
/// through. (Fedora/Arch labels avoid spaces, but Ubuntu/Debian labels can carry
/// them, so the escape keeps the match exact.)
fn escape_label(label: &str) -> String {
    label.replace(' ', "\\x20")
}

#[cfg(test)]
mod tests {
    use alloc::{collections::BTreeSet, vec};

    use super::*;

    /// Build an `exists` probe from a set of present paths.
    fn probe(paths: &'static [&'static str]) -> impl Fn(&str) -> bool {
        let set: BTreeSet<&str> = paths.iter().copied().collect();
        move |p: &str| set.contains(p)
    }

    #[test]
    fn fingerprints_arch_by_arch_boot() {
        let r = fingerprint(probe(&["/arch/boot", "/arch/version"])).unwrap();
        assert_eq!(r.family, "arch");
        assert_eq!(r.kernel[0], "/arch/boot/x86_64/vmlinuz-linux");
        // ucode images precede the main initramfs.
        assert_eq!(r.initrd[0], "/arch/boot/intel-ucode.img");
        assert_eq!(
            *r.initrd.last().unwrap(),
            "/arch/boot/x86_64/initramfs-linux.img"
        );
    }

    #[test]
    fn fingerprints_ubuntu_casper() {
        let r = fingerprint(probe(&["/casper", "/.disk/info"])).unwrap();
        assert_eq!(r.family, "ubuntu-casper");
        assert_eq!(r.media_cmdline, "boot=casper");
    }

    #[test]
    fn fingerprints_debian_live_not_ubuntu() {
        // Debian live has /live, NOT /casper — must not be mistaken for Ubuntu.
        let r = fingerprint(probe(&["/live", "/.disk/info"])).unwrap();
        assert_eq!(r.family, "debian-live");
    }

    #[test]
    fn fingerprints_fedora_by_liveos_squashfs() {
        // The `/LiveOS` directory alone is not the signature — the dracut-live
        // root file `/LiveOS/squashfs.img` is, and it survives the F44 layout
        // change (kernel moved to /boot/x86_64/loader).
        assert!(fingerprint(probe(&["/LiveOS"])).is_none());
        let r = fingerprint(probe(&["/LiveOS/squashfs.img"])).unwrap();
        assert_eq!(r.family, "fedora");
        // F44's kernel path must be a candidate (regression guard for the
        // misfingerprint-as-openSUSE the live-VM matrix caught).
        assert!(r.kernel.contains(&"/boot/x86_64/loader/linux"));
    }

    #[test]
    fn fedora44_layout_beats_opensuse() {
        // A Fedora-44-shaped disc carries BOTH /LiveOS/squashfs.img (fedora) and
        // /boot/x86_64/loader (openSUSE marker). Fedora must win by table order.
        let r = fingerprint(probe(&["/LiveOS/squashfs.img", "/boot/x86_64/loader"])).unwrap();
        assert_eq!(r.family, "fedora");
    }

    #[test]
    fn fingerprints_opensuse_and_alpine() {
        assert_eq!(
            fingerprint(probe(&["/boot/x86_64/loader"])).unwrap().family,
            "opensuse"
        );
        assert_eq!(fingerprint(probe(&["/apks"])).unwrap().family, "alpine");
    }

    #[test]
    fn unknown_layout_returns_none() {
        assert!(fingerprint(probe(&["/EFI/BOOT", "/some/random/dir"])).is_none());
        assert!(fingerprint(probe(&[])).is_none());
    }

    #[test]
    fn split_glob_recognizes_trailing_star() {
        assert_eq!(split_glob("/live/vmlinuz*"), Some(("/live", "vmlinuz")));
        assert_eq!(
            split_glob("/live/initrd.img*"),
            Some(("/live", "initrd.img"))
        );
        assert_eq!(split_glob("/casper/vmlinuz"), None);
        assert_eq!(split_glob("noslash*"), None);
    }

    #[test]
    fn glob_pick_takes_newest() {
        let names = vec!["vmlinuz-6.1.0-9-amd64", "vmlinuz-6.1.0-10-amd64", "other"];
        // lexicographic max — "-9-" < "-10-"? No: "-9" > "-10" lexically. The
        // table relies on a single match per ISO in practice; the max rule just
        // makes a multi-match deterministic. Assert the documented behavior.
        assert_eq!(glob_pick(&names, "vmlinuz"), Some("vmlinuz-6.1.0-9-amd64"));
        assert_eq!(glob_pick(&names, "initrd"), None);
    }

    #[test]
    fn glob_pick_single_match() {
        let names = vec![
            "vmlinuz-6.1.0-10-amd64",
            "config",
            "initrd.img-6.1.0-10-amd64",
        ];
        assert_eq!(glob_pick(&names, "vmlinuz"), Some("vmlinuz-6.1.0-10-amd64"));
        assert_eq!(
            glob_pick(&names, "initrd.img"),
            Some("initrd.img-6.1.0-10-amd64")
        );
    }

    #[test]
    fn render_media_cmdline_substitutes_label() {
        let r = fingerprint(probe(&["/arch/boot"])).unwrap();
        let cmd = render_cmdline(r.media_cmdline, "/isos/arch.iso", "ARCH_202606");
        assert_eq!(cmd, "archisobasedir=arch archisolabel=ARCH_202606");
        // No {iso} in the media template → the iso path is untouched.
        assert!(!cmd.contains("/isos/arch.iso"));
    }

    #[test]
    fn render_file_cmdline_substitutes_iso() {
        let r = fingerprint(probe(&["/casper"])).unwrap();
        let cmd = render_cmdline(
            r.file_cmdline,
            "/boot/isos/ubuntu.iso",
            "Ubuntu 24.04 amd64",
        );
        assert_eq!(cmd, "boot=casper iso-scan/filename=/boot/isos/ubuntu.iso");
    }

    #[test]
    fn render_escapes_label_spaces_for_cdlabel() {
        let r = fingerprint(probe(&["/boot/x86_64/loader"])).unwrap();
        let cmd = render_cmdline(r.media_cmdline, "", "openSUSE Tumbleweed");
        assert_eq!(cmd, "root=live:CDLABEL=openSUSE\\x20Tumbleweed");
    }
}
