//! Hypervisor detection via CPUID.
//!
//! Detects whether LamBoot is running inside a virtual machine and
//! identifies the hypervisor vendor. Uses CPUID leaf 0x1 bit 31
//! (hypervisor present) and leaf 0x40000000 (vendor signature).
//!
//! x86_64 only — aarch64 returns "not detected" gracefully.

use alloc::string::String;

/// Hypervisor detection result
#[derive(Debug, Clone)]
pub(crate) struct HypervisorInfo {
    pub present: bool,
    pub name: Option<String>,
}

/// Detect hypervisor presence and identify vendor via CPUID.
#[cfg(target_arch = "x86_64")]
pub(crate) fn detect_hypervisor() -> HypervisorInfo {
    // CPUID leaf 0x1, ECX bit 31 = hypervisor present
    let (_, _, ecx, _) = cpuid(0x01, 0);
    if ecx & 0x8000_0000 == 0 {
        return HypervisorInfo {
            present: false,
            name: None,
        };
    }

    // CPUID leaf 0x40000000: hypervisor vendor signature in EBX:ECX:EDX
    let (_, ebx, ecx, edx) = cpuid(0x4000_0000, 0);
    let mut sig = [0u8; 12];
    sig[0..4].copy_from_slice(&ebx.to_le_bytes());
    sig[4..8].copy_from_slice(&ecx.to_le_bytes());
    sig[8..12].copy_from_slice(&edx.to_le_bytes());

    let name = match &sig {
        b"KVMKVMKVM\0\0\0" => "KVM",
        b"Microsoft Hv" => "Hyper-V",
        b"VMwareVMware" => "VMware",
        b"XenVMMXenVMM" => "Xen",
        b" lrpepyh \0\0\0" => "Parallels",
        b"VBoxVBoxVBox" => "VirtualBox",
        _ => {
            // Return raw signature as fallback
            let s = core::str::from_utf8(&sig)
                .unwrap_or("unknown")
                .trim_end_matches('\0');
            if s.is_empty() {
                return HypervisorInfo {
                    present: true,
                    name: None,
                };
            }
            return HypervisorInfo {
                present: true,
                name: Some(String::from(s)),
            };
        }
    };

    HypervisorInfo {
        present: true,
        name: Some(String::from(name)),
    }
}

#[cfg(not(target_arch = "x86_64"))]
pub(crate) fn detect_hypervisor() -> HypervisorInfo {
    HypervisorInfo {
        present: false,
        name: None,
    }
}

/// Execute CPUID instruction with leaf (EAX) and subleaf (ECX).
/// Returns (EAX, EBX, ECX, EDX).
#[cfg(target_arch = "x86_64")]
fn cpuid(leaf: u32, subleaf: u32) -> (u32, u32, u32, u32) {
    let eax: u32;
    let ebx: u32;
    let ecx: u32;
    let edx: u32;
    // SAFETY: CPUID is a read-only instruction with no side effects.
    // It's always available on x86_64 processors.
    // rbx is LLVM-reserved, so we save/restore it manually.
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "mov {ebx_out:e}, ebx",
            "pop rbx",
            inout("eax") leaf => eax,
            ebx_out = out(reg) ebx,
            inout("ecx") subleaf => ecx,
            inout("edx") 0u32 => edx,
            options(preserves_flags),
        );
    }
    (eax, ebx, ecx, edx)
}
