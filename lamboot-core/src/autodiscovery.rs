//! Autodiscovery enhancements for the boot entry pipeline.
//!
//! Provides initrd auto-discovery, kernel path cross-referencing,
//! and stale entry sort treatment.

use alloc::{format, string::String, vec::Vec};

use crate::fs::Volume;

/// Search for initrd files matching a kernel version on a volume.
/// Tries common distro-specific patterns.
pub(crate) fn find_initrd(kernel_path: &str, volume: &mut Volume) -> Vec<String> {
    let version = extract_kernel_version(kernel_path);
    let dir = extract_directory(kernel_path);

    // Distro-specific patterns in priority order
    let patterns = [
        format!("{dir}\\initramfs-{version}.img"),
        format!("{dir}\\initrd.img-{version}"),
        format!("{dir}\\initrd-{version}"),
        format!("{dir}\\initramfs-{version}-fallback.img"),
        // Try /boot relative if kernel is in a subdirectory
        format!("\\boot\\initramfs-{version}.img"),
        format!("\\boot\\initrd.img-{version}"),
        format!("\\boot\\initrd-{version}"),
        // Root-level fallback
        format!("\\initramfs-{version}.img"),
        format!("\\initrd.img-{version}"),
    ];

    let mut found = Vec::new();

    // Check for microcode first (prepend if found)
    for ucode in ["\\intel-ucode.img", "\\amd-ucode.img"] {
        let ucode_path = format!("{dir}{ucode}");
        if volume.exists_str(&ucode_path) {
            found.push(ucode_path);
            break;
        }
    }

    // Find main initrd
    for pattern in &patterns {
        if volume.exists_str(pattern) {
            found.push(pattern.clone());
            return found;
        }
    }

    // Arch Linux special case
    if version == "linux" || version.starts_with("linux-") {
        let arch_initrd = format!("{dir}\\initramfs-linux.img");
        if volume.exists_str(&arch_initrd) {
            found.push(arch_initrd);
            return found;
        }
    }

    found
}

/// Try to resolve a kernel path that doesn't exist by searching alternative volumes.
#[expect(
    dead_code,
    reason = "disabled until extra volume scanning is re-enabled with type filtering"
)]
pub(crate) fn resolve_kernel_path(broken_path: &str, volumes: &mut [Volume]) -> Option<String> {
    let version = extract_kernel_version(broken_path);

    // Alternative paths to try
    let alternatives = [
        format!("\\boot\\vmlinuz-{version}"),
        format!("\\vmlinuz-{version}"),
        format!("\\{version}\\linux"),
        format!("\\{version}\\vmlinuz"),
    ];

    for vol in volumes.iter_mut() {
        for alt in &alternatives {
            if vol.exists_str(alt) {
                log::info!("Resolved kernel path: {broken_path} -> {alt}");
                return Some(alt.clone());
            }
        }
    }

    None
}

/// Extract version string from a kernel path.
/// "/boot/vmlinuz-6.12.0-200.fc43.x86_64" → "6.12.0-200.fc43.x86_64"
/// "/vmlinuz-linux" → "linux"
fn extract_kernel_version(path: &str) -> &str {
    let filename = path.rsplit('\\').next().unwrap_or(path);

    if let Some(ver) = filename.strip_prefix("vmlinuz-") {
        ver
    } else if let Some(ver) = filename.strip_prefix("linux-") {
        ver
    } else {
        filename
    }
}

/// Try to identify the OS on a mounted volume by reading /etc/os-release.
/// Returns (pretty_name, id) if found.
#[expect(
    dead_code,
    reason = "disabled until extra volume scanning is re-enabled with type filtering"
)]
pub(crate) fn identify_os(volume: &mut Volume) -> Option<(String, String)> {
    // Try standard path first, then fallback
    let content = volume
        .read_to_string_str("/etc/os-release")
        .or_else(|_| volume.read_to_string_str("/usr/lib/os-release"))
        .ok()?;

    let metadata = crate::uki::parse_os_release_text(&content);
    let name = metadata.0?;
    let id = metadata.1.unwrap_or_else(|| String::from("linux"));

    Some((name, id))
}

/// Extract directory from a path.
/// "\\boot\\vmlinuz-6.12.0" → "\\boot"
/// "\\vmlinuz-6.12.0" → ""
fn extract_directory(path: &str) -> &str {
    match path.rfind('\\') {
        Some(0) | None => "",
        Some(pos) => &path[..pos],
    }
}
