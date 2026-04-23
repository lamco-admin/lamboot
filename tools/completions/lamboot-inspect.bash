# bash completion for lamboot-inspect
#
# Install to /etc/bash_completion.d/ or source from ~/.bashrc.
# Honours $LAMBOOT_DIAG_ESP_MOUNT if set.

_lamboot_inspect() {
    local cur prev words cword
    _init_completion || return

    local subcommands="trust-log boot-log summary show verify dump"
    local global_opts="--help --version"

    # Top-level completion.
    if [[ $cword -eq 1 ]]; then
        COMPREPLY=($(compgen -W "$subcommands $global_opts" -- "$cur"))
        return
    fi

    local sub="${words[1]}"

    case "$sub" in
        trust-log)
            case "$prev" in
                -p|--path)
                    _filedir
                    return
                    ;;
                -f|--format)
                    COMPREPLY=($(compgen -W "text json timeline stats" -- "$cur"))
                    return
                    ;;
                -e|--event)
                    COMPREPLY=($(compgen -W "\
boot_start volume_mounted shimlock_acquired shimlock_absent \
shim_retain_requested policy_loaded policy_invalid entries_discovered \
entry_selected kernel_bytes_read image_verified kernel_measured \
cmdline_measured image_loaded_native image_load_failed initrd_registered \
driver_loaded driver_rejected boot_attempt kernel_load_failed tpm_absent" \
                        -- "$cur"))
                    return
                    ;;
            esac
            COMPREPLY=($(compgen -W "--path --format --event --errors-only --no-sha --strict --help" -- "$cur"))
            ;;
        boot-log)
            case "$prev" in
                -p|--path)
                    _filedir log
                    return
                    ;;
                -l|--level)
                    COMPREPLY=($(compgen -W "DEBUG INFO WARN ERROR" -- "$cur"))
                    return
                    ;;
                -f|--format)
                    COMPREPLY=($(compgen -W "text json" -- "$cur"))
                    return
                    ;;
            esac
            COMPREPLY=($(compgen -W "--path --level --format --errors-only --help" -- "$cur"))
            ;;
        summary)
            case "$prev" in
                --trust-path|--boot-path|--report-path|--audit-path)
                    _filedir
                    return
                    ;;
            esac
            COMPREPLY=($(compgen -W "--trust-path --boot-path --report-path --audit-path --help" -- "$cur"))
            ;;
        show)
            case "$prev" in
                -p|--path)
                    _filedir
                    return
                    ;;
            esac
            if [[ $cword -eq 2 ]]; then
                # Offer event names as the first positional.
                COMPREPLY=($(compgen -W "\
boot_start volume_mounted image_verified image_loaded_native boot_attempt \
kernel_load_failed image_load_failed driver_loaded" -- "$cur"))
                return
            fi
            COMPREPLY=($(compgen -W "--path --help" -- "$cur"))
            ;;
        verify)
            case "$prev" in
                --repo)
                    _filedir -d
                    return
                    ;;
            esac
            COMPREPLY=($(compgen -W "--repo --verbose --help" -- "$cur"))
            ;;
        dump)
            case "$prev" in
                -o|--output)
                    _filedir
                    return
                    ;;
                --esp)
                    _filedir -d
                    return
                    ;;
            esac
            COMPREPLY=($(compgen -W "--output --esp --print-manifest --help" -- "$cur"))
            ;;
    esac
}
complete -F _lamboot_inspect lamboot-inspect
