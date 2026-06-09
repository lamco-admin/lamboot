#!/usr/bin/env bats
#
# warn_if_esp_on_separate_disk surfaces the durable-NVRAM risk: when the ESP is
# on a different physical disk than the OS root, a named UEFI Boot#### entry can
# be pruned by a hypervisor's firmware (Proxmox/QEMU OVMF) if that disk isn't in
# the VM's boot order — leaving LamBoot reachable only via the
# \EFI\BOOT\BOOTX64.EFI fallback. Discovered live on Tumbleweed (VM 102).
#
# The function early-returns off a UEFI runtime (no efivars), so these tests skip
# rather than fail on a non-UEFI CI host.

load helper

setup() {
    setup_mock_path
    load_installer
    [ -d /sys/firmware/efi/efivars ] || skip "needs a UEFI runtime (no /sys/firmware/efi/efivars)"

    # findmnt -o SOURCE / => root is on sda2 (btrfs snapper decoration included);
    # lsblk PKNAME => parent disk sda; running under kvm.
    printf '#!/usr/bin/env bash\necho "/dev/sda2[/@/.snapshots/1/snapshot]"\n' > "${MOCK_BIN}/findmnt"
    printf '#!/usr/bin/env bash\necho sda\n' > "${MOCK_BIN}/lsblk"
    printf '#!/usr/bin/env bash\necho kvm\n' > "${MOCK_BIN}/systemd-detect-virt"
    chmod +x "${MOCK_BIN}/findmnt" "${MOCK_BIN}/lsblk" "${MOCK_BIN}/systemd-detect-virt"

    WARNINGS=()
    warn() { WARNINGS+=("$*"); }
    emit_event() { :; }
}

@test "sec: warns (with boot-order fix guidance) when the ESP is on a separate disk" {
    ESP_DISK="/dev/sdb"
    warn_if_esp_on_separate_disk
    printf '%s\n' "${WARNINGS[@]}" | grep -q "separate disk (/dev/sdb)"
    printf '%s\n' "${WARNINGS[@]}" | grep -q "boot order"
}

@test "flex: silent when the ESP shares the OS root disk (durable named entry)" {
    ESP_DISK="/dev/sda"
    warn_if_esp_on_separate_disk
    [ "${#WARNINGS[@]}" -eq 0 ]
}
