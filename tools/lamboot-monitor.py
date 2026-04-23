#!/usr/bin/env python3
"""
lamboot-monitor — Proxmox boot health monitoring tool.

Reads LamBoot UEFI variables from VM OVMF_VARS files to detect boot loops
and unhealthy VMs. Runs on the Proxmox host.

Usage:
    lamboot-monitor [--json] [--alert-webhook URL] [--threshold N]

Requires: qemu-nbd, access to Proxmox VM configuration.
"""

import argparse
import json
import os
import re
import struct
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass, asdict
from pathlib import Path
from typing import Optional

# Shared fleet configuration, authoritative schema documented in
# ~/lamboot-tools-dev/docs/SPEC-LAMBOOT-TOOLKIT-V1.md §16 Appendix C.
FLEET_TOML_PATH = "/etc/lamboot/fleet.toml"

# LamBoot vendor GUID: 4C414D42-4F4F-5400-0000-000000000001
LAMBOOT_GUID = "4c414d42-4f4f-5400-0000-000000000001"

# Variable names to extract
VARIABLES = {
    "LamBootState": "u8",
    "LamBootCrashCount": "u8",
    "LamBootLastEntry": "utf8",
    "LamBootTimestamp": "timestamp",
    "LamBootVersion": "u32",
}

STATE_NAMES = {0: "Fresh", 1: "Booting", 2: "BootedOK", 3: "CrashLoop"}


@dataclass
class VmHealth:
    vmid: int
    name: str
    state: str
    crash_count: int
    last_entry: str
    timestamp: str
    version: str
    status: str  # "healthy", "warning", "critical"
    qmp_status: str = "unknown"  # QMP VM status: "running", "stopped", etc.


def find_ovmf_vms() -> list[dict]:
    """Find all VMs with OVMF (UEFI) firmware on this Proxmox host."""
    vms = []
    conf_dir = Path("/etc/pve/qemu-server")
    if not conf_dir.exists():
        print("ERROR: Not a Proxmox host (no /etc/pve/qemu-server)", file=sys.stderr)
        sys.exit(1)

    for conf_file in sorted(conf_dir.glob("*.conf")):
        vmid = int(conf_file.stem)
        config = conf_file.read_text()

        # Check for OVMF BIOS
        if "bios: ovmf" not in config:
            continue

        # Find efidisk path
        efidisk_match = re.search(r"efidisk0:\s*(\S+)", config)
        if not efidisk_match:
            continue

        # Extract storage:volume from efidisk0 value
        efidisk_spec = efidisk_match.group(1).split(",")[0]

        # Get VM name
        name_match = re.search(r"name:\s*(.+)", config)
        name = name_match.group(1).strip() if name_match else f"VM {vmid}"

        vms.append({"vmid": vmid, "name": name, "efidisk": efidisk_spec})

    return vms


def resolve_efidisk_path(efidisk_spec: str) -> Optional[str]:
    """Resolve a Proxmox storage:volume spec to an actual file path."""
    # Common patterns:
    # local-lvm:vm-100-disk-1 → /dev/pve/vm-100-disk-1
    # local:100/vm-100-disk-0.qcow2 → /var/lib/vz/images/100/vm-100-disk-0.qcow2
    # local-zfs:vm-100-disk-0 → /dev/zvol/rpool/data/vm-100-disk-0

    try:
        result = subprocess.run(
            ["pvesm", "path", efidisk_spec],
            capture_output=True, text=True, timeout=10,
        )
        if result.returncode == 0:
            return result.stdout.strip()
    except (subprocess.TimeoutExpired, FileNotFoundError):
        pass

    return None


def try_virt_fw_vars(disk_path: str) -> Optional[dict]:
    """Try to extract variables using virt-fw-vars (PVE 9+, cleaner method)."""
    try:
        result = subprocess.run(
            ["virt-fw-vars", "--input", disk_path,
             "--print-guid", LAMBOOT_GUID],
            capture_output=True, text=True, timeout=10,
        )
        if result.returncode != 0:
            return None

        # virt-fw-vars outputs variable info as text
        variables = {}
        for line in result.stdout.splitlines():
            line = line.strip()
            for var_name in VARIABLES:
                if var_name in line:
                    # Parse the hex data after the variable name
                    parts = line.split(":")
                    if len(parts) >= 2:
                        hex_data = parts[-1].strip().replace(" ", "")
                        if hex_data:
                            raw = bytes.fromhex(hex_data)
                            var_type = VARIABLES[var_name]
                            if var_type == "u8" and len(raw) >= 1:
                                variables[var_name] = raw[0]
                            elif var_type == "u32" and len(raw) >= 4:
                                variables[var_name] = struct.unpack_from("<I", raw)[0]
                            elif var_type == "utf8":
                                variables[var_name] = raw.decode("utf-8", errors="replace").rstrip("\x00")
        return variables if variables else None
    except (FileNotFoundError, subprocess.TimeoutExpired):
        return None


def query_qmp_status(vmid: int) -> Optional[dict]:
    """Query VM status via QMP socket (running VMs only)."""
    qmp_socket = f"/var/run/qemu-server/{vmid}.qmp"
    if not os.path.exists(qmp_socket):
        return None

    try:
        import socket as _socket
        sock = _socket.socket(_socket.AF_UNIX, _socket.SOCK_STREAM)
        sock.settimeout(5)
        sock.connect(qmp_socket)

        # Read greeting
        sock.recv(4096)

        # Send capabilities negotiation
        sock.sendall(b'{"execute": "qmp_capabilities"}\n')
        sock.recv(4096)

        # Query status
        sock.sendall(b'{"execute": "query-status"}\n')
        response = sock.recv(4096)
        sock.close()

        data = json.loads(response)
        if "return" in data:
            return data["return"]
    except Exception:
        pass

    return None


def extract_uefi_vars(disk_path: str) -> dict:
    """Extract LamBoot UEFI variables from an OVMF_VARS disk image.
    Tries virt-fw-vars first (PVE 9+), falls back to binary search."""
    variables = {}

    # Try virt-fw-vars first (cleaner, handles qcow2)
    vfw_result = try_virt_fw_vars(disk_path)
    if vfw_result is not None:
        return vfw_result

    # Fallback: direct binary search of raw OVMF_VARS files
    try:
        with open(disk_path, "rb") as f:
            data = f.read()
    except (OSError, PermissionError) as e:
        return {"_error": str(e)}

    # Search for LamBoot GUID in the variable store
    # GUID is stored in mixed-endian format in UEFI:
    # First 3 groups are little-endian, last 2 are big-endian
    guid_bytes = guid_to_bytes(LAMBOOT_GUID)

    pos = 0
    while True:
        pos = data.find(guid_bytes, pos)
        if pos == -1:
            break

        # Try to extract variable data around this GUID occurrence
        # UEFI variable format varies, but we can look for known patterns
        for var_name, var_type in VARIABLES.items():
            var_name_utf16 = var_name.encode("utf-16-le")
            # Search near the GUID for the variable name
            search_start = max(0, pos - 256)
            search_end = min(len(data), pos + 256)
            region = data[search_start:search_end]

            name_pos = region.find(var_name_utf16)
            if name_pos == -1:
                continue

            # Variable data follows the name + null terminator
            data_offset = search_start + name_pos + len(var_name_utf16) + 2
            if data_offset >= len(data):
                continue

            if var_type == "u8" and data_offset < len(data):
                variables[var_name] = data[data_offset]
            elif var_type == "u32" and data_offset + 4 <= len(data):
                variables[var_name] = struct.unpack_from("<I", data, data_offset)[0]
            elif var_type == "utf8" and data_offset + 1 < len(data):
                end = data.find(b"\x00", data_offset, data_offset + 128)
                if end > data_offset:
                    variables[var_name] = data[data_offset:end].decode("utf-8", errors="replace")
            elif var_type == "timestamp" and data_offset + 8 <= len(data):
                ts = struct.unpack_from("<HBBBBBB", data, data_offset)
                if ts[0] > 2000:  # Sanity check year
                    variables[var_name] = f"{ts[0]:04d}-{ts[1]:02d}-{ts[2]:02d}T{ts[3]:02d}:{ts[4]:02d}:{ts[5]:02d}"

        pos += 16  # Move past this GUID occurrence

    return variables


def guid_to_bytes(guid_str: str) -> bytes:
    """Convert GUID string to UEFI mixed-endian byte representation."""
    parts = guid_str.split("-")
    # First 3 groups: little-endian
    b = struct.pack("<IHH", int(parts[0], 16), int(parts[1], 16), int(parts[2], 16))
    # Last 2 groups: big-endian (just raw hex bytes)
    b += bytes.fromhex(parts[3])
    b += bytes.fromhex(parts[4])
    return b


def assess_health(variables: dict, threshold: int = 2) -> VmHealth:
    """Assess VM boot health from extracted variables."""
    state_val = variables.get("LamBootState", 0)
    crash_count = variables.get("LamBootCrashCount", 0)
    last_entry = variables.get("LamBootLastEntry", "unknown")
    timestamp = variables.get("LamBootTimestamp", "unknown")

    version_int = variables.get("LamBootVersion", 0)
    if version_int:
        major = (version_int >> 16) & 0xFF
        minor = (version_int >> 8) & 0xFF
        patch = version_int & 0xFF
        version = f"{major}.{minor}.{patch}"
    else:
        version = "unknown"

    state = STATE_NAMES.get(state_val, f"Unknown({state_val})")

    # Determine health status
    if state_val == 3:  # CrashLoop
        status = "critical"
    elif state_val == 1 and crash_count >= threshold:  # Stuck in Booting with high count
        status = "critical"
    elif state_val == 1:  # Booting (may be in progress)
        status = "warning"
    elif crash_count > 0:  # Some crashes but not in loop
        status = "warning"
    else:
        status = "healthy"

    return VmHealth(
        vmid=0, name="", state=state, crash_count=crash_count,
        last_entry=last_entry, timestamp=timestamp, version=version,
        status=status,
    )


def send_webhook(url: str, payload: dict):
    """Send alert via webhook (POST JSON)."""
    try:
        import urllib.request
        req = urllib.request.Request(
            url,
            data=json.dumps(payload).encode(),
            headers={"Content-Type": "application/json"},
        )
        urllib.request.urlopen(req, timeout=10)
    except Exception as e:
        print(f"Webhook failed: {e}", file=sys.stderr)


def load_fleet_config(path: str = FLEET_TOML_PATH) -> dict:
    """Return the [monitor] section of /etc/lamboot/fleet.toml, or {} if absent.

    Schema v1 is defined in ~/lamboot-tools-dev/docs/SPEC-LAMBOOT-TOOLKIT-V1.md
    §16 Appendix C. This function is tolerant: missing file, missing TOML
    parser, missing section, or wrong schema version all return {} so the
    caller can fall back to hardcoded defaults.
    """
    if not os.path.exists(path):
        return {}

    # Python 3.11+: stdlib tomllib. Older: tomli (optional). Fail soft.
    toml_loader = None
    try:
        import tomllib as toml_loader  # type: ignore[import-not-found,no-redef]
    except ImportError:
        try:
            import tomli as toml_loader  # type: ignore[import-not-found,no-redef]
        except ImportError:
            print(
                f"WARN: {path} present but neither tomllib nor tomli is "
                "installed; ignoring. Install python3-tomli on older distros.",
                file=sys.stderr,
            )
            return {}

    try:
        with open(path, "rb") as f:
            data = toml_loader.load(f)
    except Exception as e:  # TOMLDecodeError in 3.11+, other exceptions otherwise
        print(f"WARN: failed to parse {path}: {e}; ignoring.", file=sys.stderr)
        return {}

    schema = data.get("schema_version")
    if schema not in (None, 1):
        print(
            f"WARN: {path} schema_version={schema} is not 1; ignoring "
            "(update lamboot-dev or downgrade fleet.toml).",
            file=sys.stderr,
        )
        return {}

    monitor = data.get("monitor", {})
    if not isinstance(monitor, dict):
        return {}
    return monitor


def main():
    # Read /etc/lamboot/fleet.toml [monitor] section to seed argparse defaults.
    # CLI flags always win over the file; the file wins over hardcoded defaults.
    fleet_monitor = load_fleet_config()
    default_webhook = fleet_monitor.get("alert_webhook") or None
    default_log_path = fleet_monitor.get("log_path") or "/var/log/lamboot-monitor.log"

    parser = argparse.ArgumentParser(description="LamBoot Proxmox boot health monitor")
    parser.add_argument("--json", action="store_true", help="Output as JSON")
    parser.add_argument("--threshold", type=int, default=2, help="Crash count threshold (default: 2)")
    parser.add_argument(
        "--alert-webhook",
        default=default_webhook,
        help=(
            "Webhook URL for critical alerts "
            f"(default: from {FLEET_TOML_PATH} [monitor].alert_webhook if set)"
        ),
    )
    parser.add_argument(
        "--log-path",
        default=default_log_path,
        help=(
            "Log file path "
            f"(reserved; currently unused. Default from {FLEET_TOML_PATH} [monitor].log_path)"
        ),
    )
    parser.add_argument("--vmid", type=int, help="Check a specific VM only")
    parser.add_argument("--fleet-config", help="Override fleet.toml path for debugging")
    args = parser.parse_args()

    # Re-read with override if --fleet-config given (rare; testing only).
    if args.fleet_config:
        override = load_fleet_config(args.fleet_config)
        if args.alert_webhook is None:
            args.alert_webhook = override.get("alert_webhook")

    # Reject plain-http webhooks per schema validation rule (§16 Appendix C).
    if args.alert_webhook and not args.alert_webhook.startswith("https://"):
        print(
            f"ERROR: alert webhook must use HTTPS: {args.alert_webhook}",
            file=sys.stderr,
        )
        sys.exit(2)

    vms = find_ovmf_vms()
    if args.vmid:
        vms = [v for v in vms if v["vmid"] == args.vmid]

    if not vms:
        print("No OVMF VMs found" + (f" (vmid={args.vmid})" if args.vmid else ""))
        sys.exit(0)

    results = []
    for vm in vms:
        disk_path = resolve_efidisk_path(vm["efidisk"])
        if not disk_path:
            if not args.json:
                print(f"  SKIP VM {vm['vmid']} ({vm['name']}): cannot resolve efidisk path")
            continue

        variables = extract_uefi_vars(disk_path)
        if "_error" in variables:
            if not args.json:
                print(f"  SKIP VM {vm['vmid']} ({vm['name']}): {variables['_error']}")
            continue

        if not variables:
            # No LamBoot variables found — not using LamBoot
            continue

        health = assess_health(variables, args.threshold)
        health.vmid = vm["vmid"]
        health.name = vm["name"]

        # Enrich with QMP status if VM is running
        qmp = query_qmp_status(vm["vmid"])
        if qmp:
            health.qmp_status = qmp.get("status", "unknown")
        else:
            health.qmp_status = "stopped"

        results.append(health)

    if args.json:
        print(json.dumps([asdict(r) for r in results], indent=2))
    else:
        if not results:
            print("No VMs with LamBoot detected.")
            return

        print(f"{'VMID':>6}  {'Status':>8}  {'State':>10}  {'Crashes':>7}  {'Name'}")
        print("-" * 60)
        for r in results:
            status_marker = {"healthy": "OK", "warning": "WARN", "critical": "CRIT"}[r.status]
            print(f"{r.vmid:>6}  {status_marker:>8}  {r.state:>10}  {r.crash_count:>7}  {r.name}")

    # Send alerts for critical VMs
    critical = [r for r in results if r.status == "critical"]
    if critical and args.alert_webhook:
        send_webhook(args.alert_webhook, {
            "alert": "lamboot-crash-loop",
            "vms": [asdict(r) for r in critical],
            "timestamp": time.strftime("%Y-%m-%dT%H:%M:%S"),
        })


if __name__ == "__main__":
    main()
