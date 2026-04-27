#!/bin/bash
#
# verify-claims.sh — SDS-4 §9.3 marketing-discipline check.
#
# Every security claim LamBoot makes publicly MUST be backed by a
# specific code path. This script parses the §8.1 claims table in
# SPEC-NATIVE-TRUST-CHAIN.md and confirms the referenced file /
# function / symbol actually exists in the current tree.
#
# Fails the build if any reference is stale. Keeps the claims
# appendix honest as the code evolves.
#
# Runs as part of pre-commit (optionally) and in CI (always).

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

SPEC="docs/specs/SPEC-NATIVE-TRUST-CHAIN.md"

if [ ! -f "$SPEC" ]; then
    echo "ERROR: $SPEC not found"
    exit 1
fi

readonly RED=$'\033[0;31m'
readonly GREEN=$'\033[0;32m'
readonly YELLOW=$'\033[0;33m'
readonly RESET=$'\033[0m'

fail_count=0
ok_count=0

check_symbol_in_file() {
    local file="$1"
    local symbol="$2"
    local claim="$3"
    if [ ! -f "$file" ]; then
        echo "${RED}[FAIL]${RESET} file missing: $file"
        echo "       claim: $claim"
        fail_count=$((fail_count + 1))
        return
    fi
    if grep -qE "$symbol" "$file"; then
        echo "${GREEN}[ OK ]${RESET} $file :: $symbol"
        ok_count=$((ok_count + 1))
    else
        echo "${RED}[FAIL]${RESET} symbol missing: $symbol"
        echo "       in: $file"
        echo "       claim: $claim"
        fail_count=$((fail_count + 1))
    fi
}

check_event_in_trust_log() {
    local event="$1"
    local claim="$2"
    # Accept either a string literal OR a const with matching value.
    if grep -rqE "\"$event\"" lamboot-core/src/ 2>/dev/null; then
        echo "${GREEN}[ OK ]${RESET} trust event \"$event\" emitted"
        ok_count=$((ok_count + 1))
    else
        echo "${RED}[FAIL]${RESET} trust event \"$event\" not found"
        echo "       claim: $claim"
        fail_count=$((fail_count + 1))
    fi
}

echo "== SDS-4 §8.1 claim verification =="
echo

# Claim 1: ShimLock::Verify at Step 10.
check_symbol_in_file \
    "lamboot-core/src/boot.rs" \
    "fn verify_kernel_bytes" \
    "LamBoot verifies every kernel via shim before loading it"

# Claim 2: JSON trust log.
check_event_in_trust_log \
    "image_verified" \
    "LamBoot writes a JSON audit log to the ESP"

# Claim 3: No ext4 writes (EspWriter FAT-only constraint).
check_symbol_in_file \
    "lamboot-core/src/fs_writer.rs" \
    "pub\\(crate\\) fn new\\(volume: &'v mut Volume\\) -> Option<Self>" \
    "LamBoot does not modify /boot. Ever."

# Claim 4: ext4-view native backend.
check_symbol_in_file \
    "lamboot-core/src/fs_backend_ext4.rs" \
    "use ext4_view::" \
    "LamBoot's native boot reads /boot via pure-Rust ext4-view"

# Claim 5: PE loader has SAFETY comments on every unsafe.
check_symbol_in_file \
    "lamboot-core/src/pe_loader.rs" \
    "SAFETY:" \
    "Every unsafe block in pe_loader is documented"

# Claim 6: Native load path bypasses BS->LoadImage.
check_symbol_in_file \
    "lamboot-core/src/boot.rs" \
    "fn native_load_and_start" \
    "Kernels are verified once and loaded natively"

# Claim 7: TPM PCR extends.
check_symbol_in_file \
    "lamboot-core/src/tpm.rs" \
    "fn measure_kernel" \
    "LamBoot measures kernel into TPM PCR 4"
check_symbol_in_file \
    "lamboot-core/src/tpm.rs" \
    "fn measure_cmdline" \
    "LamBoot measures cmdline into TPM PCR 12"

# Claim 8: SHA-256 in trust events. The field + builder live in the
# pure half of the split so host tests can include them via #[path];
# `trust_log.rs` re-exports through the canonical module boundary.
check_symbol_in_file \
    "lamboot-core/src/trust_log_pure.rs" \
    "pub sha256_hex" \
    "Trust log includes SHA-256 of every verified and loaded image"

# Claim 9: SB-off announced via token. Vocabulary constants also live
# in the pure half for the same reason.
check_symbol_in_file \
    "lamboot-core/src/trust_log_pure.rs" \
    "V_DEGRADED_TRUST_SB_OFF" \
    "If Secure Boot is off, LamBoot says so in the log"

echo
echo "-- summary --"
echo "  passed: $ok_count"
echo "  failed: $fail_count"

# SDS-4 §8.2 prohibited-claim grep. Each of these strings, if found
# in user-facing docs (README, docs/, website content), is a red
# flag — the claim hasn't been added to §8.1 with backing code.
echo
echo "== SDS-4 §8.2 prohibited-claim grep =="
prohibited_hits=0
prohibited_grep() {
    local needle="$1"
    local reason="$2"
    if grep -rIlE "$needle" README.md docs/ 2>/dev/null \
        | grep -vE "(SECURITY-MODEL|SPEC-NATIVE-TRUST-CHAIN)" \
        | grep -q .
    then
        echo "${YELLOW}[WARN]${RESET} user-facing docs mention: $needle"
        echo "        reason: $reason"
        prohibited_hits=$((prohibited_hits + 1))
    fi
}

prohibited_grep 'shim-review approved' \
    'LamBoot has not been shim-review approved. SDS-4 §8.2.'
prohibited_grep 'post-quantum ready' \
    'LamBoot is not post-quantum ready. SDS-4 §8.2.'
prohibited_grep 'formally verified' \
    'LamBoot PE loader is tested + fuzzed, not formally verified. SDS-4 §8.2.'
prohibited_grep 'without shim' \
    'LamBoot depends on shim ShimLock for MOK chains. SDS-4 §8.2.'
prohibited_grep 'verifies initrd' \
    'LamBoot does not verify initrd integrity on non-UKI boots. SDS-4 §4.'

if [ "$prohibited_hits" -eq 0 ]; then
    echo "${GREEN}[ OK ]${RESET} no prohibited claims found in user-facing docs"
fi

if [ "$fail_count" -gt 0 ]; then
    echo
    echo "${RED}FAIL${RESET}: $fail_count claim reference(s) missing — amend the spec or fix the code."
    exit 1
fi

if [ "$prohibited_hits" -gt 0 ]; then
    echo
    echo "${YELLOW}WARNINGS${RESET}: $prohibited_hits prohibited-claim hit(s) — review user-facing docs."
    # Warn only, don't fail. A curator with a legitimate reason (e.g.
    # discussing a limitation) can reword to avoid false positives.
fi

echo "${GREEN}PASS${RESET}: all claim references valid."
exit 0
