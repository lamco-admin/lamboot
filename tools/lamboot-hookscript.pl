#!/usr/bin/perl
# LamBoot Proxmox Hookscript
#
# Integrates LamBoot boot health monitoring with Proxmox VM lifecycle.
# Captures boot health at post-stop and optionally resets state at pre-start.
#
# Install:
#   cp lamboot-hookscript.pl /var/lib/vz/snippets/
#   chmod +x /var/lib/vz/snippets/lamboot-hookscript.pl
#   qm set <vmid> --hookscript local:snippets/lamboot-hookscript.pl
#
# Requires: lamboot-monitor.py installed at /usr/local/bin/

use strict;
use warnings;
use POSIX qw(strftime);

my $MONITOR = '/usr/local/bin/lamboot-monitor.py';
my $LOG_DIR = '/var/log/lamboot';
my $FLEET_LOG = "$LOG_DIR/fleet.jsonl";

# Proxmox hookscript receives: vmid phase
my $vmid = shift @ARGV || die "Usage: $0 <vmid> <phase>\n";
my $phase = shift @ARGV || die "Usage: $0 <vmid> <phase>\n";

# Ensure log directory exists
unless (-d $LOG_DIR) {
    mkdir $LOG_DIR, 0755 or warn "Cannot create $LOG_DIR: $!\n";
}

if ($phase eq 'pre-start') {
    handle_pre_start($vmid);
}
elsif ($phase eq 'post-start') {
    handle_post_start($vmid);
}
elsif ($phase eq 'pre-stop') {
    # Nothing to do pre-stop
}
elsif ($phase eq 'post-stop') {
    handle_post_stop($vmid);
}

exit 0;

sub handle_pre_start {
    my ($vmid) = @_;
    log_event($vmid, 'pre-start', 'VM starting');

    # Auto-inject VMID into SMBIOS OEM strings so LamBoot can display it.
    # Reads current args, adds -smbios type=11 if not already present.
    ensure_vmid_smbios($vmid);

    # Check previous boot health before starting
    if (-x $MONITOR) {
        my $output = `$MONITOR --vmid $vmid --json 2>/dev/null`;
        if ($? == 0 && $output) {
            chomp $output;
            append_fleet_log($output);

            if ($output =~ /"status"\s*:\s*"critical"/) {
                log_event($vmid, 'pre-start',
                    'WARNING: Previous boot was in crash loop state');
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

    # Capture boot health from OVMF_VARS
    if (-x $MONITOR) {
        my $output = `$MONITOR --vmid $vmid --json 2>/dev/null`;
        if ($? == 0 && $output) {
            chomp $output;
            append_fleet_log($output);
            log_event($vmid, 'post-stop', "Boot health captured");
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

sub ensure_vmid_smbios {
    my ($vmid) = @_;
    my $conf_file = "/etc/pve/qemu-server/$vmid.conf";
    my $smbios_value = "lamboot.vmid=$vmid";

    # Read current config to check if already set
    open my $fh, '<', $conf_file or do {
        log_event($vmid, 'pre-start', "Cannot read $conf_file: $!");
        return;
    };
    my @lines = <$fh>;
    close $fh;

    # Check if VMID is already in the args line
    my $has_vmid = 0;
    my $has_args = 0;
    for my $line (@lines) {
        if ($line =~ /^args:/) {
            $has_args = 1;
            if ($line =~ /lamboot\.vmid=/) {
                $has_vmid = 1;
            }
        }
    }

    return if $has_vmid;

    my $smbios_arg = "-smbios type=11,value=$smbios_value";

    if ($has_args) {
        # Append to existing args line
        system("qm", "set", $vmid, "--args",
            get_current_args($conf_file) . " $smbios_arg");
    }
    else {
        system("qm", "set", $vmid, "--args", $smbios_arg);
    }

    if ($? == 0) {
        log_event($vmid, 'pre-start', "Injected SMBIOS VMID: $smbios_value");
    }
    else {
        log_event($vmid, 'pre-start', "Failed to inject SMBIOS VMID");
    }
}

sub get_current_args {
    my ($conf_file) = @_;
    open my $fh, '<', $conf_file or return '';
    while (<$fh>) {
        if (/^args:\s*(.+)/) {
            close $fh;
            my $args = $1;
            chomp $args;
            return $args;
        }
    }
    close $fh;
    return '';
}

sub log_event {
    my ($vmid, $phase, $message) = @_;
    my $timestamp = strftime('%Y-%m-%dT%H:%M:%S', localtime);
    my $log_file = "$LOG_DIR/hookscript.log";

    if (open my $fh, '>>', $log_file) {
        print $fh "[$timestamp] VM $vmid ($phase): $message\n";
        close $fh;
    }
}

sub append_fleet_log {
    my ($json_line) = @_;
    if (open my $fh, '>>', $FLEET_LOG) {
        print $fh "$json_line\n";
        close $fh;
    }
}
