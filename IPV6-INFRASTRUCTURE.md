# IPv6 Infrastructure Documentation

## Overview

This document describes the IPv6 network infrastructure for the lamco home lab.
Last updated: 2026-03-26.

## Network Topology

```
Spectrum ISP (Charter)
    |
    | DHCPv6 (IA_NA + IA_PD)
    |
[Nighthawk RAX50] (192.168.1.1)
    | WAN: 2600:6c42:7005:x:x:x:x:x/64 (dynamic, ISP-assigned)
    | LAN: 2600:6c42:6700:d3:120c:6bff:fed5:f106/64
    | Role: IPv6 gateway, prefix delegation, default router
    | Mode: DHCP (Auto Detect selected DHCP)
    |
    +-- [ns1] (192.168.1.53 / 2600:6c42:6700:d3:be24:11ff:fe21:84ad)
    |   Role: DNS (PowerDNS), DHCP4+6 (Kea), RA (radvd), DDNS
    |
    +-- [pve1] (192.168.1.160 / 2600:6c42:6700:d3:da43:aeff:fe4d:d864)
    |   Role: Proxmox hypervisor
    |
    +-- [aibox] (192.168.1.88 / 2600:6c42:6700:d3:56ac:e586:e98a:152b)
    |   Role: Dev workstation (VM on pve1)
    |
    +-- ~30 other devices
```

## IPv6 Prefix

- **ISP allocation**: `2600:6c42:6700::/48` (Spectrum/Charter)
- **LAN prefix**: `2600:6c42:6700:d3::/64` (delegated via DHCPv6-PD to Nighthawk)
- **WAN prefix**: `2600:6c42:7005::/64` (ISP link network, changes on reconnect)

**WARNING**: The ISP prefix can change when the DHCPv6-PD lease expires or the
modem/router is rebooted. If the LAN prefix changes, the following must be updated:
- Kea DHCPv6 config: `/etc/kea/kea-dhcp6.conf` (subnet and pool)
- PowerDNS AAAA records (manually or via DDNS)
- radvd config: `/etc/radvd.conf` (prefix)
- PowerDNS recursor: `/etc/powerdns/recursor.yml` (listen, allow_from, forward-zones)
- Nighthawk router: IPv6 DNS field (Primary DNS)

## Services on ns1 (192.168.1.53)

### Router Advertisements (radvd)

**Config**: `/etc/radvd.conf`

radvd sends RAs on the LAN with:
- `AdvManagedFlag on` (M=1) — clients should use DHCPv6 for addresses
- `AdvOtherConfigFlag on` (O=1) — clients should use DHCPv6 for other config
- `AdvDefaultLifetime 0` — ns1 is NOT a default router (Nighthawk handles routing)
- `AdvAutonomous on` — clients also use SLAAC (dual addressing)
- RDNSS pointing to ns1's IPv6 address
- DNSSL with `a.lamco.io`

**Sysctl** (`/etc/sysctl.d/60-ipv6-radvd.conf`):
```
net.ipv6.conf.all.forwarding = 1
net.ipv6.conf.eth0.accept_ra = 2
```
Forwarding is required for radvd. `accept_ra=2` allows ns1 to still accept
RAs from the Nighthawk (keeping its own SLAAC address) despite forwarding=1.

### DHCPv6 (Kea)

**Config**: `/etc/kea/kea-dhcp6.conf`

- Subnet: `2600:6c42:6700:d3::/64`
- Pool: `2600:6c42:6700:d3::1000 - 2600:6c42:6700:d3::1fff` (4096 addresses)
- Interface: `eth0` (required for subnet selection — without this, Kea can't
  match link-local client addresses to the subnet)
- Lease backend: **memfile** (`/var/lib/kea/kea-leases6.csv`)
  - PostgreSQL was disabled due to data race bug in Kea's AllocEngine with
    multi-threading (PACKET_DROP_0007). Matches DHCPv4 which also uses memfile.
- DDNS updates enabled via `kea-dhcp-ddns`
- DNS server option: ns1's IPv6 address

### DHCPv4 (Kea)

**Config**: `/etc/kea/kea-dhcp4.conf`

- Subnet: `192.168.1.0/24`
- Pool: `192.168.1.110 - 192.168.1.149`
- Lease backend: memfile (`/var/lib/kea/kea-leases4.csv`)
- 25+ host reservations
- DDNS updates enabled

### DHCP-DDNS (Kea)

**Config**: `/etc/kea/kea-dhcp-ddns.conf`

Bridges DHCP lease events to DNS updates:
- Forward zone: `a.lamco.io.` → 127.0.0.1:5300
- Reverse zone (IPv4): `1.168.192.in-addr.arpa.` → 127.0.0.1:5300
- Reverse zone (IPv6): `3.d.0.0.0.0.7.6.2.4.c.6.0.0.6.2.ip6.arpa.` → 127.0.0.1:5300

Uses `check-with-dhcid` conflict resolution — prevents DHCPv6 from overwriting
records created by DHCPv4 (and vice versa) for the same hostname.

### PowerDNS Authoritative

**Config**: `/etc/powerdns/pdns.conf`
**Backend**: PostgreSQL (`pdns` database)
**Port**: 5300 (localhost only)

Zones:
- `a.lamco.io` — forward zone (A, AAAA, CNAME, DHCID records)
- `1.168.192.in-addr.arpa` — IPv4 reverse zone
- `3.d.0.0.0.0.7.6.2.4.c.6.0.0.6.2.ip6.arpa` — IPv6 reverse zone

DNSUPDATE enabled for DDNS from Kea.

### PowerDNS Recursor

**Config**: `/etc/powerdns/recursor.yml`
**Port**: 53

Listens on:
- `192.168.1.53:53` (IPv4)
- `127.0.0.1:53` (localhost)
- `[2600:6c42:6700:d3:be24:11ff:fe21:84ad]:53` (IPv6)

Forwards local zones to authoritative at 127.0.0.1:5300:
- `a.lamco.io`
- `1.168.192.in-addr.arpa`
- `3.d.0.0.0.0.7.6.2.4.c.6.0.0.6.2.ip6.arpa`

Allows queries from:
- `192.168.1.0/24`, `127.0.0.0/8`, `::1/128`, `fe80::/10`, `2600:6c42:6700:d3::/64`

DNSSEC validation enabled with negative trust anchors for local zones.

## Nighthawk Router (RAX50) IPv6 Config

- **Connection Type**: DHCP (Auto Detect chose this)
- **"Use DHCP Server"**: Unchecked (Kea on ns1 handles DHCPv6)
- **Primary DNS**: `2600:6C42:6700:00D3:BE24:11FF:FE21:84AD` (ns1)
- **Secondary DNS**: `2001:4860:4860::8844` (Google)

The Nighthawk handles:
- WAN-side DHCPv6 (getting its own address + prefix delegation from Spectrum)
- Default routing for IPv6 traffic
- Router Advertisements on LAN (with M=0, O=0 — but radvd overrides with M=1, O=1)

## Troubleshooting

### IPv6 internet not working
1. Check router WAN IPv6 address (should not say "Not Available")
2. If no WAN address: reboot router, or power cycle modem then router
3. If WAN address present but no routing: check if LAN prefix matches a valid PD
4. `ping6 google.com` from aibox, ns1, and pve1

### DHCPv6 not allocating addresses
1. Check Kea logs: `journalctl -u isc-kea-dhcp6-server`
2. Look for `SUBNET_SELECTION_FAILED` — means `"interface": "eth0"` is missing from subnet config
3. Look for `NoAddrsAvail` in tcpdump — same cause or lease DB issue
4. Check lease file: `sudo cat /var/lib/kea/kea-leases6.csv`

### DDNS not creating records
1. Check DDNS logs: `journalctl -u isc-kea-dhcp-ddns-server`
2. RCODE 8 (rejected) usually means DHCID conflict between v4 and v6 leases
3. Check PowerDNS records: `PGPASSWORD=PowerDNS2026 psql -h 127.0.0.1 -U pdns -d pdns -c "SELECT name, type, content FROM records WHERE type IN ('AAAA','PTR') ORDER BY type, name;"`

### ISP prefix changed
If the prefix changes (visible in router's LAN IPv6 address), update all configs
listed in the PREFIX section above. This is a manual process — the RAX50 doesn't
expose prefix change notifications.
