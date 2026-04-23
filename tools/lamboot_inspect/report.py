"""Boot-report consumer.

Reads the JSON boot report written by ``lamboot-core/src/report.rs`` to
``\\EFI\\LamBoot\\reports\\boot.json``. Exposes a typed view that the
CLI ``show``/``summary`` subcommands consume.

The report schema is small and stable; we surface it as-is plus a few
derived convenience fields (e.g. an ``iommu`` boolean).
"""
from __future__ import annotations

import dataclasses
import io
import json
import os
import pathlib
from typing import Optional, Union


@dataclasses.dataclass
class BootReport:
    lamboot_version: str = ""
    lamboot_arch: str = ""
    timestamp: str = ""
    entry_id: str = ""
    entry_name: str = ""
    entry_type: str = ""
    path: str = ""
    system_manufacturer: str = ""
    system_product: str = ""
    fleet_id: str = ""
    vmid: str = ""
    os_name: str = ""
    hypervisor: str = ""
    iommu: str = ""
    iommu_units: int = 0
    boot_timing_ms: str = ""
    raw: dict = dataclasses.field(default_factory=dict)

    @property
    def iommu_present(self) -> bool:
        return bool(self.iommu)

    @property
    def entry_is_uki(self) -> bool:
        return self.entry_type == "uki"

    @property
    def entry_is_chainload(self) -> bool:
        return self.entry_type == "chainload"

    @property
    def entry_is_legacy(self) -> bool:
        return self.entry_type == "linux_legacy"


def parse(
    source: Union[str, bytes, pathlib.Path, io.IOBase],
) -> BootReport:
    if isinstance(source, (str, os.PathLike)) and not isinstance(source, bytes):
        text = pathlib.Path(source).read_text(encoding="utf-8", errors="replace")
    elif isinstance(source, bytes):
        text = source.decode("utf-8", errors="replace")
    elif hasattr(source, "read"):
        data = source.read()
        text = data.decode("utf-8", errors="replace") if isinstance(data, bytes) else data
    else:
        raise TypeError(f"unsupported source type: {type(source).__name__}")

    obj = json.loads(text)
    if not isinstance(obj, dict):
        raise ValueError("boot report root is not a JSON object")
    return BootReport(
        lamboot_version=str(obj.get("lamboot_version", "")),
        lamboot_arch=str(obj.get("lamboot_arch", "")),
        timestamp=str(obj.get("timestamp", "")),
        entry_id=str(obj.get("entry_id", "")),
        entry_name=str(obj.get("entry_name", "")),
        entry_type=str(obj.get("entry_type", "")),
        path=str(obj.get("path", "")),
        system_manufacturer=str(obj.get("system_manufacturer", "")),
        system_product=str(obj.get("system_product", "")),
        fleet_id=str(obj.get("fleet_id", "")),
        vmid=str(obj.get("vmid", "")),
        os_name=str(obj.get("os_name", "")),
        hypervisor=str(obj.get("hypervisor", "")),
        iommu=str(obj.get("iommu", "")),
        iommu_units=int(obj.get("iommu_units", 0) or 0),
        boot_timing_ms=str(obj.get("boot_timing_ms", "")),
        raw=obj,
    )
