//! TPM 2.0 measured boot via TCG2 protocol.
//!
//! Extends PCR registers with measurements of boot components:
//!   - PCR 4: Boot loader code (kernel image)
//!   - PCR 5: Boot loader configuration (policy, BLS entries)
//!   - PCR 12: Kernel command line
//!
//! Gracefully degrades when TPM is not present — never blocks boot.

use alloc::vec::Vec;

use uefi::proto::tcg::{
    v2::{HashLogExtendEventFlags, Tcg},
    EventType, PcrIndex,
};

/// PCR indices following the Linux TPM PCR Registry (UAPI Group)
const PCR_BOOT_CODE: PcrIndex = PcrIndex(4);
const PCR_BOOT_CONFIG: PcrIndex = PcrIndex(5);
const PCR_KERNEL_CMDLINE: PcrIndex = PcrIndex(12);

/// TPM measurement context. Holds a reference to the TCG2 protocol.
pub(crate) struct TpmContext {
    _available: bool,
}

impl TpmContext {
    /// Initialize TPM context. Returns a context even if TPM is absent.
    pub(crate) fn new() -> Self {
        let available = Self::check_tpm_available();
        if available {
            log::info!("TPM 2.0: available, measured boot enabled");
        } else {
            log::info!("TPM 2.0: not available, measurements skipped");
        }
        Self {
            _available: available,
        }
    }

    /// Check if TPM 2.0 is present and operational
    fn check_tpm_available() -> bool {
        let Ok(handle) = uefi::boot::get_handle_for_protocol::<Tcg>() else {
            return false;
        };

        let Ok(mut tcg) = uefi::boot::open_protocol_exclusive::<Tcg>(handle) else {
            return false;
        };

        match tcg.get_capability() {
            Ok(cap) => cap.tpm_present(),
            Err(_) => false,
        }
    }

    /// Measure a kernel image into PCR 4
    pub(crate) fn measure_kernel(&self, kernel_data: &[u8]) {
        self.measure(
            PCR_BOOT_CODE,
            EventType::EFI_BOOT_SERVICES_APPLICATION,
            HashLogExtendEventFlags::PE_COFF_IMAGE,
            kernel_data,
            b"LamBoot: Kernel Image",
        );
    }

    /// Measure boot configuration (policy file) into PCR 5
    pub(crate) fn measure_config(&self, config_data: &[u8]) {
        self.measure(
            PCR_BOOT_CONFIG,
            EventType::IPL,
            HashLogExtendEventFlags::empty(),
            config_data,
            b"LamBoot: Boot Configuration",
        );
    }

    /// Measure kernel command line into PCR 12 (UTF-16, no trailing NUL)
    pub(crate) fn measure_cmdline(&self, cmdline: &str) {
        // Convert to UTF-16 without trailing NUL (per systemd convention)
        let utf16: Vec<u16> = cmdline.encode_utf16().collect();
        let bytes: Vec<u8> = utf16.iter().flat_map(|c| c.to_le_bytes()).collect();

        self.measure(
            PCR_KERNEL_CMDLINE,
            EventType::IPL,
            HashLogExtendEventFlags::empty(),
            &bytes,
            b"LamBoot: Kernel Command Line",
        );
    }

    /// Measure a loaded driver/module binary into PCR 4
    pub(crate) fn measure_driver(&self, data: &[u8], path: &str) {
        let desc = alloc::format!("LamBoot: Driver {path}");
        self.measure(
            PCR_BOOT_CODE,
            EventType::EFI_BOOT_SERVICES_APPLICATION,
            HashLogExtendEventFlags::PE_COFF_IMAGE,
            data,
            desc.as_bytes(),
        );
    }

    /// Measure a BLS entry file into PCR 5
    pub(crate) fn measure_bls_entry(&self, entry_data: &[u8], entry_id: &str) {
        let desc = alloc::format!("LamBoot: BLS Entry {entry_id}");
        self.measure(
            PCR_BOOT_CONFIG,
            EventType::IPL,
            HashLogExtendEventFlags::empty(),
            entry_data,
            desc.as_bytes(),
        );
    }

    /// Core measurement function — hash, extend PCR, and log event.
    #[expect(
        clippy::unused_self,
        reason = "TpmContext methods keep &self for API consistency — future implementations will use stored protocol handle"
    )]
    fn measure(
        &self,
        pcr: PcrIndex,
        event_type: EventType,
        flags: HashLogExtendEventFlags,
        data: &[u8],
        description: &[u8],
    ) {
        let Ok(handle) = uefi::boot::get_handle_for_protocol::<Tcg>() else {
            return; // No TPM, silently succeed
        };

        let Ok(mut tcg) = uefi::boot::open_protocol_exclusive::<Tcg>(handle) else {
            return;
        };

        // Create event input
        let event =
            match uefi::proto::tcg::v2::PcrEventInputs::new_in_box(pcr, event_type, description) {
                Ok(e) => e,
                Err(e) => {
                    log::warn!("Failed to create PCR event: {e:?}");
                    return;
                }
            };

        // Hash, extend, and log
        match tcg.hash_log_extend_event(flags, data, &event) {
            Ok(()) => {
                log::info!("TPM: measured {} bytes into PCR {}", data.len(), pcr.0);
            }
            Err(e) => {
                log::warn!("TPM measurement failed for PCR {}: {e:?}", pcr.0);
                // Non-fatal — never block boot for TPM errors
            }
        }
    }
}
