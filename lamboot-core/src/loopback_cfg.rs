//! Layer: 3 — boot-entry parsers.
//!
//! A narrow extractor for a distro ISO's `/boot/grub/loopback.cfg` (boot-from-ISO
//! Path A1, SPEC-BOOT-FROM-ISO-DESIGN §2.4). It is **not** a grub interpreter: it
//! pulls the kernel path, the first initrd path, and the kernel command line out
//! of the first bootable `menuentry`. Because the distro's own `loopback.cfg`
//! already carries the correct iso-find token (`iso-scan/filename=`, `findiso=`,
//! `root=live:` …) pointing at `${iso_path}`, Path A1 needs no per-distro table:
//! substituting `${iso_path}` with the real path yields the right command line
//! for that distro for free.
//!
//! Pure — bytes-in, struct-out, no I/O — so it is host-tested in `lamboot-fs-tests`.

use alloc::string::{String, ToString};

/// The kernel/initrd/cmdline pulled from a `loopback.cfg` menuentry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoopbackEntry {
    /// Kernel path on the ISO (e.g. `/casper/vmlinuz`).
    pub kernel: String,
    /// First initrd path on the ISO (e.g. `/casper/initrd`).
    pub initrd: String,
    /// Kernel command line with `${iso_path}` substituted to the real ISO path.
    pub options: String,
}

/// Extract the first bootable menuentry's `linux`/`initrd`/cmdline, substituting
/// `${iso_path}` (and `$iso_path`) with `iso_path`. Returns `None` if no
/// menuentry with both a `linux` and an `initrd` line is found.
pub(crate) fn parse_first_menuentry(cfg: &str, iso_path: &str) -> Option<LoopbackEntry> {
    let mut started = false;
    let mut depth: i32 = 0;
    let mut saw_open = false;
    let mut kernel: Option<(String, String)> = None;
    let mut initrd: Option<String> = None;

    for raw in cfg.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if !started {
            // A `menuentry`/`submenu` opens the first block we care about.
            if first_word(line) == "menuentry" || first_word(line) == "submenu" {
                started = true;
                depth += count_braces(line);
                saw_open = depth > 0;
            }
            continue;
        }

        if kernel.is_none() {
            if let Some(rest) = strip_kw(line, &["linux", "linux16", "linuxefi"]) {
                let mut it = rest.splitn(2, char::is_whitespace);
                let path = it.next().unwrap_or("").to_string();
                let args = it.next().unwrap_or("").trim().to_string();
                if !path.is_empty() {
                    kernel = Some((path, args));
                }
            }
        }
        if initrd.is_none() {
            if let Some(rest) = strip_kw(line, &["initrd", "initrd16", "initrdefi"]) {
                if let Some(p) = rest.split_whitespace().next() {
                    initrd = Some(p.to_string());
                }
            }
        }

        depth += count_braces(line);
        saw_open = saw_open || depth > 0;
        // The first entry's block has closed (only meaningful once we've seen its
        // opening brace), or we already have everything we need.
        if (saw_open && depth <= 0) || (kernel.is_some() && initrd.is_some()) {
            break;
        }
    }

    let (kernel, args) = kernel?;
    let initrd = initrd?;
    Some(LoopbackEntry {
        kernel,
        initrd,
        options: substitute_iso_path(&args, iso_path),
    })
}

fn first_word(line: &str) -> &str {
    line.split_whitespace().next().unwrap_or("")
}

/// If `line` begins with one of `kws` followed by whitespace, return the trimmed
/// remainder; else `None`. (So `linuxhdr` does not match `linux`.)
fn strip_kw<'a>(line: &'a str, kws: &[&str]) -> Option<&'a str> {
    for kw in kws {
        if let Some(rest) = line.strip_prefix(kw) {
            if rest.starts_with(char::is_whitespace) {
                return Some(rest.trim_start());
            }
        }
    }
    None
}

fn count_braces(line: &str) -> i32 {
    line.chars()
        .map(|c| match c {
            '{' => 1,
            '}' => -1,
            _ => 0,
        })
        .sum()
}

fn substitute_iso_path(args: &str, iso_path: &str) -> String {
    args.replace("${iso_path}", iso_path)
        .replace("$iso_path", iso_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ubuntu_casper_first_entry() {
        let cfg = r#"
        menuentry "Try or Install Ubuntu" {
            set gfxpayload=keep
            linux /casper/vmlinuz boot=casper iso-scan/filename=${iso_path} quiet splash ---
            initrd /casper/initrd
        }
        menuentry "OEM install (for manufacturers)" {
            linux /casper/vmlinuz boot=casper oem-config/enable=true iso-scan/filename=${iso_path} ---
            initrd /casper/initrd
        }
        "#;
        let e = parse_first_menuentry(cfg, "/isos/ubuntu.iso").unwrap();
        assert_eq!(e.kernel, "/casper/vmlinuz");
        assert_eq!(e.initrd, "/casper/initrd");
        assert_eq!(
            e.options,
            "boot=casper iso-scan/filename=/isos/ubuntu.iso quiet splash ---"
        );
        // must pick the FIRST entry's args, not the OEM one
        assert!(!e.options.contains("oem-config"));
    }

    #[test]
    fn debian_live_findiso() {
        let cfg = r#"
        menuentry "Debian GNU/Linux Live" {
            linux  /live/vmlinuz-6.1.0 boot=live components findiso=${iso_path}
            initrd /live/initrd.img-6.1.0
        }
        "#;
        let e = parse_first_menuentry(cfg, "/boot/isos/debian.iso").unwrap();
        assert_eq!(e.kernel, "/live/vmlinuz-6.1.0");
        assert_eq!(e.initrd, "/live/initrd.img-6.1.0");
        assert_eq!(
            e.options,
            "boot=live components findiso=/boot/isos/debian.iso"
        );
    }

    #[test]
    fn skips_leading_non_menuentry_and_comments() {
        let cfg = "# generated\nset timeout=30\nif loadfont unicode ; then\n  set gfxmode=auto\nfi\nmenuentry 'Live' {\n linux16 /isolinux/vmlinuz root=live:CDLABEL=X iso-scan/filename=${iso_path}\n initrd16 /isolinux/initrd.img\n}\n";
        let e = parse_first_menuentry(cfg, "/x.iso").unwrap();
        assert_eq!(e.kernel, "/isolinux/vmlinuz");
        assert_eq!(e.initrd, "/isolinux/initrd.img");
        assert_eq!(e.options, "root=live:CDLABEL=X iso-scan/filename=/x.iso");
    }

    #[test]
    fn no_initrd_returns_none() {
        let cfg = "menuentry 'x' {\n linux /vmlinuz ro\n}\n";
        assert!(parse_first_menuentry(cfg, "/x.iso").is_none());
    }

    #[test]
    fn empty_or_garbage_returns_none() {
        assert!(parse_first_menuentry("", "/x.iso").is_none());
        assert!(parse_first_menuentry("just some text\nno entries", "/x.iso").is_none());
    }
}
