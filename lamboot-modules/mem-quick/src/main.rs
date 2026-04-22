#![no_main]
#![no_std]

use log::info;
use uefi::{mem::memory_map::MemoryMap, prelude::*};

#[entry]
fn efi_main() -> Status {
    uefi::helpers::init().expect("Failed to initialize UEFI");

    info!("Quick Memory Test v0.1.0");
    info!("========================");
    info!("");

    // Get memory map to find testable regions
    let mmap_size = uefi::boot::memory_map(uefi::boot::MemoryType::LOADER_DATA);
    match mmap_size {
        Ok(mmap) => {
            let mut total_bytes: u64 = 0;
            let mut conventional_bytes: u64 = 0;
            let mut regions = 0u32;

            for desc in mmap.entries() {
                let size = desc.page_count * 4096;
                total_bytes += size;

                if desc.ty == uefi::boot::MemoryType::CONVENTIONAL {
                    conventional_bytes += size;
                    regions += 1;
                }
            }

            info!("Total memory: {} MB", total_bytes / (1024 * 1024));
            info!(
                "Available (conventional): {} MB in {} regions",
                conventional_bytes / (1024 * 1024),
                regions
            );
            info!("");

            // Quick walking-ones test on a small region
            info!("Running walking-ones test...");
            let test_size = 4096; // Test 4KB
            let test_result = run_walking_ones_test(test_size);
            if test_result {
                info!("Walking-ones test: PASSED");
            } else {
                info!("Walking-ones test: FAILED");
            }

            info!("");
            info!("Running address-pattern test...");
            let addr_result = run_address_pattern_test(test_size);
            if addr_result {
                info!("Address-pattern test: PASSED");
            } else {
                info!("Address-pattern test: FAILED");
            }
        }
        Err(e) => {
            info!("Failed to get memory map: {e:?}");
        }
    }

    info!("");
    info!("Test complete. Press any key to return to LamBoot...");

    uefi::system::with_stdin(|stdin| loop {
        if stdin.read_key().ok().flatten().is_some() {
            break;
        }
        uefi::boot::stall(core::time::Duration::from_millis(10));
    });

    Status::SUCCESS
}

/// Walking-ones test: write and verify shifting bit patterns
fn run_walking_ones_test(size: usize) -> bool {
    // Allocate test buffer
    let mut buffer = alloc::vec![0u8; size];

    for bit in 0..8 {
        let pattern = 1u8 << bit;
        // Write pattern
        buffer.fill(pattern);
        // Verify
        for (i, byte) in buffer.iter().enumerate() {
            if *byte != pattern {
                log::error!("Mismatch at offset {i}: expected {pattern:#04x}, got {byte:#04x}");
                return false;
            }
        }
    }
    true
}

/// Address-pattern test: write address-derived values and verify
fn run_address_pattern_test(size: usize) -> bool {
    let mut buffer = alloc::vec![0u8; size];

    // Write: each byte = (address % 251) — prime to avoid power-of-2 aliasing
    for (i, byte) in buffer.iter_mut().enumerate() {
        *byte = (i % 251) as u8;
    }

    // Verify
    for (i, byte) in buffer.iter().enumerate() {
        let expected = (i % 251) as u8;
        if *byte != expected {
            log::error!("Mismatch at offset {i}: expected {expected:#04x}, got {byte:#04x}");
            return false;
        }
    }
    true
}

extern crate alloc;
