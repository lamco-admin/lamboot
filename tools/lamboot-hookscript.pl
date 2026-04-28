#!/usr/bin/perl
# LamBoot Proxmox Hookscript
# version: 0.8.4
#
# Refreshes the per-VM configuration JSON at pre-start and captures boot
# health at post-stop. Reads /etc/lamboot/fleet.toml (schema v1) to
# determine fleet ID and per-VM role, and emits the shared per-VM JSON
# schema v1 (matching what lamboot-pve-setup writes at setup time) to
# /var/lib/lamboot/<VMID>.json.
#
# This file is configured permanently on each VM by lamboot-pve-setup via
# a single `args:` line containing
#     -fw_cfg name=opt/lamboot/config,file=/var/lib/lamboot/<VMID>.json
# The hookscript therefore does NOT call `qm set` at any lifecycle event —
# the args line is set once, at setup time, on a stopped VM. QEMU then
# exposes the JSON file via fw_cfg on every boot; the hookscript's only
# job here is to refresh the file's contents so the guest sees current
# fleet metadata.
#
# Prior behavior (pre-0.8.4) mutated VM config via `qm set --args` during
# pre-start, which silently failed because Proxmox config-locks the VM
# configuration during that lifecycle phase.
#
# Install:
#   cp lamboot-hookscript.pl /var/lib/vz/snippets/
#   chmod +x /var/lib/vz/snippets/lamboot-hookscript.pl
#   # Then `sudo lamboot-pve-setup setup <VMID>` on each VM (from the
#   # lamboot-toolkit-pve package) to attach this hookscript + the
#   # permanent args: line + write initial per-VM JSON.
#
# Requires: lamboot-monitor.py installed at /usr/local/bin/ (for post-stop
# boot-health capture). Python 3 with either tomllib (stdlib in 3.11+) or
# tomli (fallback) for fleet.toml parsing.

use strict;
use warnings;
use POSIX qw(strftime);
use File::Temp qw(tempfile);
use File::Basename qw(dirname);
use File::Path qw(make_path);

my $HOOKSCRIPT_VERSION = '0.8.4';

my $MONITOR       = '/usr/local/bin/lamboot-monitor.py';
my $LOG_DIR       = '/var/log/lamboot';
my $FLEET_LOG     = "$LOG_DIR/fleet.jsonl";
my $HOOKSCRIPT_LOG = "$LOG_DIR/hookscript.log";
my $FLEET_TOML    = '/etc/lamboot/fleet.toml';
my $STATE_DIR     = '/var/lib/lamboot';
my $SCHEMA_VERSION = 'v1';

# Proxmox hookscript args: vmid phase
my $vmid  = shift @ARGV || die "Usage: $0 <vmid> <phase>\n";
my $phase = shift @ARGV || die "Usage: $0 <vmid> <phase>\n";

# Ensure log directory exists (best-effort; never fatal).
for my $dir ($LOG_DIR) {
    unless (-d $dir) {
        mkdir $dir, 0755 or warn "Cannot create $dir: $!\n";
    }
}

if ($phase eq 'pre-start') {
    handle_pre_start($vmid);
}
elsif ($phase eq 'post-start') {
    handle_post_start($vmid);
}
elsif ($phase eq 'pre-stop') {
    # No action; included for completeness.
}
elsif ($phase eq 'post-stop') {
    handle_post_stop($vmid);
}

exit 0;

# ----------------------------------------------------------------------
# Lifecycle handlers
# ----------------------------------------------------------------------

sub handle_pre_start {
    my ($vmid) = @_;
    log_event($vmid, 'pre-start', 'VM starting');

    # Refresh the per-VM JSON exposed via fw_cfg. This is the primary
    # work the new hookscript does — replaces the old SMBIOS-via-qm-set
    # trick that deadlocked on config lock.
    my $fleet = load_fleet_config($FLEET_TOML);
    my $hooks = $fleet->{hookscript} // {};

    my %vm_meta = read_vm_config($vmid);
    my $role    = determine_role($vmid, $fleet, \%vm_meta, $hooks);

    my $json_file = "$STATE_DIR/$vmid.json";
    if (should_inject_any($hooks)) {
        ensure_state_dir();
        my $ok = write_per_vm_json(
            path      => $json_file,
            vmid      => $vmid,
            hostname  => safe_hostname(),
            fleet_id  => $fleet->{fleet}{id},
            role      => $role,
            tags      => $vm_meta{tags},
            hooks     => $hooks,
        );
        if ($ok) {
            log_event($vmid, 'pre-start',
                "Refreshed $json_file (fleet=" . ($fleet->{fleet}{id} // 'none')
                . " role=" . ($role // 'none') . ")");
        }
        else {
            log_event($vmid, 'pre-start', "Failed to refresh $json_file (continuing)");
        }
    }
    else {
        log_event($vmid, 'pre-start',
            'All inject_* flags disabled in fleet.toml [hookscript]; skipping JSON refresh');
    }

    # Previous-boot health check. Capture any crash-loop state so the
    # fleet log has a pre-start record before the new boot begins.
    if (-x $MONITOR) {
        my $output = capture_monitor($vmid);
        if (defined $output && length $output) {
            append_fleet_log($output);
            if ($output =~ /"status"\s*:\s*"critical"/) {
                log_event($vmid, 'pre-start',
                    'WARNING: previous boot was in crash-loop state');
            }
        }
    }
}

sub handle_post_start {
    my ($vmid) = @_;
    log_event($vmid, 'post-start', 'VM started');
}

sub handle_post_stop {
    my ($vmid) = @_;
    log_event($vmid, 'post-stop', 'VM stopped, capturing boot health');

    if (-x $MONITOR) {
        my $output = capture_monitor($vmid);
        if (defined $output && length $output) {
            append_fleet_log($output);
            log_event($vmid, 'post-stop', 'Boot health captured');
        }
        else {
            log_event($vmid, 'post-stop',
                'Failed to capture boot health (monitor returned error)');
        }
    }
    else {
        log_event($vmid, 'post-stop',
            "lamboot-monitor.py not found at $MONITOR");
    }
}

# ----------------------------------------------------------------------
# Fleet config + VM metadata readers
# ----------------------------------------------------------------------

# Shell out to Python to parse the TOML. Proxmox ships Python 3; tomllib
# is stdlib on 3.11+, tomli is a widely-available fallback. Returns a
# hashref (empty if file missing, parse fails, or schema mismatched).
sub load_fleet_config {
    my ($path) = @_;
    return {} unless -r $path;

    my $py = <<'PY';
import json, sys
try:
    import tomllib
except ImportError:
    try:
        import tomli as tomllib
    except ImportError:
        print('{}')
        sys.exit(0)
path = sys.argv[1]
try:
    with open(path, 'rb') as f:
        data = tomllib.load(f)
except Exception:
    print('{}')
    sys.exit(0)
schema = data.get('schema_version')
if schema not in (None, 1):
    print('{}')
    sys.exit(0)
print(json.dumps(data))
PY

    my ($fh, $tmp) = tempfile('lamboot-hook-toml-XXXXXX', TMPDIR => 1, UNLINK => 1);
    print $fh $py;
    close $fh;

    my @cmd = ('python3', $tmp, $path);
    my $json = qx{@cmd 2>/dev/null};
    my $rc   = $? >> 8;
    return {} if $rc != 0 || !defined $json || $json !~ /^\s*\{/;

    my $parsed = decode_json_safe($json);
    return ref $parsed eq 'HASH' ? $parsed : {};
}

sub read_vm_config {
    my ($vmid) = @_;
    my $conf = "/etc/pve/qemu-server/$vmid.conf";
    my %meta = (name => '', tags => []);
    return %meta unless -r $conf;

    open my $fh, '<', $conf or return %meta;
    while (my $line = <$fh>) {
        chomp $line;
        if ($line =~ /^name:\s*(.+)$/) {
            $meta{name} = $1;
        }
        elsif ($line =~ /^tags:\s*(.+)$/) {
            my @tags = grep { length } split /[;,]/, $1;
            $meta{tags} = [ map { my $t = $_; $t =~ s/^\s+|\s+$//g; $t } @tags ];
        }
    }
    close $fh;
    return %meta;
}

sub determine_role {
    my ($vmid, $fleet, $vm_meta, $hooks) = @_;

    # inject_role off → no role written, even if configured.
    return undef if exists $hooks->{inject_role} && !$hooks->{inject_role};

    # Explicit [roles]."<VMID>" override wins.
    if (ref $fleet->{roles} eq 'HASH') {
        my $explicit = $fleet->{roles}{$vmid} // $fleet->{roles}{"$vmid"};
        return $explicit if defined $explicit && length $explicit;
    }

    # Tag-based role matching (first match wins, stable-ordered).
    if (ref $fleet->{tags} eq 'HASH' && ref $vm_meta->{tags} eq 'ARRAY') {
        my %vm_tags = map { $_ => 1 } @{ $vm_meta->{tags} };
        for my $role_name (sort keys %{ $fleet->{tags} }) {
            my $tag_list = $fleet->{tags}{$role_name};
            next unless ref $tag_list eq 'ARRAY';
            for my $tag (@$tag_list) {
                return $role_name if $vm_tags{$tag};
            }
        }
    }

    return undef;
}

sub should_inject_any {
    my ($hooks) = @_;
    # Defaults when [hookscript] section absent: inject everything.
    return 1 unless ref $hooks eq 'HASH' && %$hooks;
    my $a = $hooks->{inject_fleet_id} // 1;
    my $b = $hooks->{inject_role}     // 1;
    my $c = $hooks->{inject_vmid}     // 1;
    return ($a || $b || $c) ? 1 : 0;
}

# ----------------------------------------------------------------------
# Per-VM JSON writer — matches lamboot-pve-setup schema v1 exactly
# ----------------------------------------------------------------------

sub ensure_state_dir {
    return if -d $STATE_DIR;
    make_path($STATE_DIR, { mode => 0755 }) or warn "Cannot create $STATE_DIR: $!";
}

sub write_per_vm_json {
    my %a = @_;

    my $hooks       = $a{hooks} // {};
    my $inject_vmid = $hooks->{inject_vmid}     // 1;
    my $inject_fid  = $hooks->{inject_fleet_id} // 1;
    my $inject_role = $hooks->{inject_role}     // 1;

    my @lines;
    push @lines, '{';
    push @lines, qq(  "schema_version": ) . json_str($SCHEMA_VERSION) . ',';

    push @lines, qq(  "vmid": ) . json_str($a{vmid}) . ','  if $inject_vmid;
    push @lines, qq(  "hostname": ) . json_str($a{hostname}) . ',';
    push @lines, qq(  "fleet_id": ) . json_str($a{fleet_id} // '') . ',' if $inject_fid;
    push @lines, qq(  "role": ) . json_str($a{role} // '') . ',' if $inject_role;
    push @lines, qq(  "written_by": ) . json_str("lamboot-hookscript $HOOKSCRIPT_VERSION") . ',';
    push @lines, qq(  "written_at": ) . json_str(iso_utc_now()) . ',';

    my $tags = $a{tags} // [];
    my $tags_json = '[' . join(',', map { json_str($_) } @$tags) . ']';
    push @lines, qq(  "tags_at_setup": $tags_json);

    push @lines, '}';

    my $body = join("\n", @lines) . "\n";

    # Atomic write: tempfile in same dir + rename.
    my $dir = dirname($a{path});
    my ($tfh, $tfile) = tempfile("$a{vmid}.json.XXXXXX", DIR => $dir, UNLINK => 0);
    print $tfh $body;
    close $tfh;
    chmod 0644, $tfile;
    unless (rename $tfile, $a{path}) {
        warn "rename $tfile -> $a{path} failed: $!";
        unlink $tfile;
        return 0;
    }
    return 1;
}

# ----------------------------------------------------------------------
# Utilities
# ----------------------------------------------------------------------

sub capture_monitor {
    my ($vmid) = @_;
    my $output = qx{$MONITOR --vmid $vmid --json 2>/dev/null};
    return undef if $? != 0 || !defined $output;
    chomp $output;
    return $output;
}

sub append_fleet_log {
    my ($line) = @_;
    if (open my $fh, '>>', $FLEET_LOG) {
        print $fh "$line\n";
        close $fh;
    }
}

sub log_event {
    my ($vmid, $phase, $message) = @_;
    my $ts = strftime('%Y-%m-%dT%H:%M:%S', localtime);
    if (open my $fh, '>>', $HOOKSCRIPT_LOG) {
        print $fh "[$ts] VM $vmid ($phase): $message\n";
        close $fh;
    }
}

sub safe_hostname {
    my $h = qx{hostname 2>/dev/null};
    chomp $h if defined $h;
    return $h // 'unknown';
}

sub iso_utc_now {
    return strftime('%Y-%m-%dT%H:%M:%SZ', gmtime);
}

# JSON string emitter — escapes the subset of characters JSON actually
# requires. Avoids pulling in JSON::PP as a hard dependency even though
# Perl 5.14+ ships it (we stay core-free for maximum portability).
sub json_str {
    my ($s) = @_;
    $s = '' unless defined $s;
    $s =~ s/\\/\\\\/g;
    $s =~ s/"/\\"/g;
    $s =~ s/\x08/\\b/g;
    $s =~ s/\x0c/\\f/g;
    $s =~ s/\n/\\n/g;
    $s =~ s/\r/\\r/g;
    $s =~ s/\t/\\t/g;
    $s =~ s/([\x00-\x1f])/sprintf('\\u%04x', ord($1))/ge;
    return qq("$s");
}

# Minimal JSON decoder for hashref trees. Uses JSON::PP if available
# (Perl 5.14+ ships it), falls back to a conservative inline parser
# sufficient for the python-emitted TOML dict we consume.
sub decode_json_safe {
    my ($s) = @_;
    return {} unless defined $s;
    if (eval { require JSON::PP; 1 }) {
        my $obj = eval { JSON::PP::decode_json($s) };
        return ref $obj eq 'HASH' ? $obj : {};
    }
    # No JSON::PP — extremely unusual on Perl 5.14+; return empty.
    return {};
}
