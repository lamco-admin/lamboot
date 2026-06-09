#!/bin/bash
# Package LamBoot release tarball.
#
# Outputs:
#   dist/lamboot-<VERSION>-<ARCH>.tar.gz
#   dist/lamboot-<VERSION>-<ARCH>.tar.gz.sha256
#
# Prerequisites:
#   - ./build.sh completed (signed + unsigned binaries present in dist/)
#   - ./tools/sign-lamboot.sh completed (signed drivers + modules present)
#   - dist/OVMF_VARS_lamboot.fd built with production keys
#   - Cargo.toml version matches the tarball version

set -euo pipefail

VERSION=$(awk -F'"' '/^version/ {print $2; exit}' Cargo.toml)
ARCH="x86_64"
STAGING="/tmp/lamboot-release-${VERSION}"
TARBALL_NAME="lamboot-${VERSION}-${ARCH}.tar.gz"
OUTPUT_DIR="${1:-dist}"

echo "══ Packaging LamBoot v${VERSION} (${ARCH}) ══"

# Sanity checks
REQUIRED=(
    dist/EFI/LamBoot/lambootx64.efi
    dist/EFI/LamBoot/lambootx64-signed.efi
    dist/EFI/LamBoot/db.der
    dist/EFI/LamBoot/drivers/ext4_x64.efi
    dist/EFI/LamBoot/drivers/ext4_x64-signed.efi
    dist/EFI/LamBoot/modules/manifest.toml
    dist/EFI/LamBoot/policy.toml
    dist/OVMF_VARS_lamboot.fd
    dist/kernel-hooks/zz-lamboot-postinst
    dist/kernel-hooks/zz-lamboot-postrm
    tools/lamboot-install
    lib/esp-deploy.sh
    CHANGELOG.md
    SECURITY.md
    LICENSE-MIT
    LICENSE-APACHE
    packaging/tarball-README.md
)

missing=0
for f in "${REQUIRED[@]}"; do
    if [[ ! -f "$f" ]]; then
        echo "❌ Missing: $f"
        missing=1
    fi
done
if (( missing )); then
    echo
    echo "Refusing to package. Fix missing artifacts first."
    exit 1
fi
echo "✓ All required artifacts present"

# Build staging tree
rm -rf "$STAGING"
mkdir -p "$STAGING/lamboot-${VERSION}"
cd "$STAGING/lamboot-${VERSION}"

# Install script + supporting tools
cp -a /home/greg/lamboot-dev/tools/lamboot-install ./lamboot-install
cp -a /home/greg/lamboot-dev/tools/lamboot-kernel-hook ./lamboot-kernel-hook
cp -a /home/greg/lamboot-dev/tools/lamboot-monitor.py ./lamboot-monitor.py
# v0.11.13 — Proxmox-host runtime metrics sampler
cp -a /home/greg/lamboot-dev/tools/lamboot-host-sampler ./lamboot-host-sampler 2>/dev/null || true
cp -a /home/greg/lamboot-dev/tools/sign-lamboot.sh ./sign-lamboot.sh
cp -a /home/greg/lamboot-dev/tools/sign-unlock ./sign-unlock
cp -a /home/greg/lamboot-dev/tools/sign-lock ./sign-lock
cp -a /home/greg/lamboot-dev/tools/build-ovmf-vars.sh ./build-ovmf-vars.sh 2>/dev/null || true

# lamboot-inspect — diagnostic suite (trust log / boot log / verify / dump).
# Bundled with the implementation package so the script can import
# lamboot_inspect.cli without an install step.
cp -a /home/greg/lamboot-dev/tools/lamboot-inspect ./lamboot-inspect
cp -a /home/greg/lamboot-dev/tools/lamboot_inspect ./lamboot_inspect
mkdir -p man/man1 completions
# Generate the man page from lamboot-inspect's help registry (--dump-registry)
# rather than copying a tracked .1 — the tracked page drifted to 0.8.3 once. The
# vendored generator is byte-compatible with the toolkit's, so the lamboot and
# lamboot-tools packages ship an identical inspect(1).
/home/greg/lamboot-dev/tools/registry-to-man \
    /home/greg/lamboot-dev/tools/lamboot-inspect ./man/man1 >/dev/null
cp -a /home/greg/lamboot-dev/tools/completions/lamboot-inspect.bash ./completions/lamboot-inspect.bash
cp -a /home/greg/lamboot-dev/tools/completions/_lamboot-inspect ./completions/_lamboot-inspect

# Shell library — canonical ESP file-layout primitives.
# lamboot-install sources this from <self-dir>/lib/esp-deploy.sh
# (third candidate in its lib search order); shipping it here is what
# makes the tarball self-contained for fresh extracts.
mkdir -p lib
cp -a /home/greg/lamboot-dev/lib/esp-deploy.sh ./lib/esp-deploy.sh

# EFI artifacts
mkdir -p EFI/LamBoot
cp -a /home/greg/lamboot-dev/dist/EFI/LamBoot/* EFI/LamBoot/

# OVMF VARS (Proxmox zero-touch, Config 4)
cp /home/greg/lamboot-dev/dist/OVMF_VARS_lamboot.fd ./OVMF_VARS_lamboot.fd

# Signing cert (public, for MOK enrollment by users)
cp /home/greg/lamboot-dev/dist/EFI/LamBoot/db.der ./lamboot-signing.der

# Kernel-install plugin + kernel hooks
mkdir -p kernel-install kernel-hooks
cp -a /home/greg/lamboot-dev/dist/kernel-install/* kernel-install/ 2>/dev/null || true
cp -a /home/greg/lamboot-dev/dist/kernel-hooks/* kernel-hooks/ 2>/dev/null || true

# Systemd
mkdir -p systemd
cp -a /home/greg/lamboot-dev/dist/systemd/* systemd/ 2>/dev/null || true

# v0.11.15 — archinstall plugin + AUR alpm hook. PKGBUILD's package()
# references these paths inside the extracted tarball (lines that copy
# packaging/installers/archinstall/ and packaging/aur/lamboot/
# 95-lamboot.hook). Before v0.11.15 the tarball did not stage these
# and the AUR build would have failed on those install -Dm0644 lines.
mkdir -p packaging/installers/archinstall packaging/installers/calamares packaging/installers/openSUSE
mkdir -p packaging/aur/lamboot

cp -a /home/greg/lamboot-dev/packaging/installers/archinstall/. \
    packaging/installers/archinstall/ 2>/dev/null || true
cp -a /home/greg/lamboot-dev/packaging/installers/calamares/. \
    packaging/installers/calamares/ 2>/dev/null || true
cp -a /home/greg/lamboot-dev/packaging/installers/openSUSE/. \
    packaging/installers/openSUSE/ 2>/dev/null || true
cp -a /home/greg/lamboot-dev/packaging/aur/lamboot/95-lamboot.hook \
    packaging/aur/lamboot/95-lamboot.hook 2>/dev/null || true

# Documentation — THIN tarball (S1, 2026-06-08). The -bin tarball ships only the
# legal files + CHANGELOG + SECURITY + a STUB README that points to the canonical
# docs at lamco.ai. The full docs/ tree is intentionally NOT bundled: it lives in
# the public repo and on the website, so a docs/README content or link change no
# longer forces a tarball rebuild + re-sign. See packaging/PACKAGING.md
# "three publish tracks" (Code / Packaging-metadata / Presentation).
cp /home/greg/lamboot-dev/CHANGELOG.md ./CHANGELOG.md
cp /home/greg/lamboot-dev/SECURITY.md ./SECURITY.md
cp /home/greg/lamboot-dev/packaging/tarball-README.md ./README.md
cp /home/greg/lamboot-dev/LICENSE-MIT ./LICENSE-MIT
cp /home/greg/lamboot-dev/LICENSE-APACHE ./LICENSE-APACHE

# ── Cross-reference scan ───────────────────────────────────────────────
# THIN-tarball model (S1): docs are no longer bundled — they live on the website
# and in the public repo. The allow-list below is the set of PUBLIC doc names
# that may legitimately be referenced (a bundled tool or CHANGELOG entry naming
# one is fine; the user follows it to lamco.ai / the repo). Any docs/<NAME>.md
# reference NOT in this list is an INTERNAL-doc leak and aborts the build. New
# user-facing references should prefer a https://lamco.ai/... URL (which this
# scan does not flag) over a docs/<NAME>.md path.
echo
echo "══ Cross-reference scan ══"
TARBALL_DOCS=(
    SECURE-BOOT-DEPLOYMENT.md
    SECURE-BOOT-AND-SIGNING-STRATEGY.md
    SECURITY-MODEL.md
    SECURITY-GUIDE.md
    MOK-ENROLLMENT-GUIDE.md
    OVMF-VARS-PROXMOX.md
    PROXMOX-GUIDE.md
    PROXMOX-GUEST-INTEGRATION-LAYER.md
    KEY-GENERATION.md
    INSTALL-REFERENCE.md
    CONFIGURATION-GUIDE.md
    TROUBLESHOOTING-GUIDE.md
    USER-GUIDE.md
    DEVELOPER-GUIDE.md
    DIAGNOSTIC-MODULES.md
    ARCHITECTURE.md
    ARCHITECTURE-LAYERS.md
    LAMBOOT-TOOLS-OVERVIEW.md
    LAMBOOT-INSPECT.md
)
declare -A _ALLOWED_DOC=()
for _d in "${TARBALL_DOCS[@]}"; do _ALLOWED_DOC["$_d"]=1; done

_LEAK_REPORT=""
while IFS= read -r -d '' _f; do
    while IFS=: read -r _line _ref; do
        _name="${_ref#docs/}"
        if [[ -z "${_ALLOWED_DOC[$_name]:-}" ]]; then
            _rel="${_f#$STAGING/}"
            _LEAK_REPORT+="  ${_rel}:${_line}: refers to docs/${_name} (NOT in tarball)"$'\n'
        fi
    done < <(grep -niEo '\bdocs/[A-Z][A-Z0-9_.-]*\.md\b' "$_f" 2>/dev/null | head -100)
done < <(find "$STAGING" -type f \
            \( -name '*.md' -o -name 'LICENSE' -o -name 'LICENSE-*' \
               -o -name 'CONTRIBUTING' -o -name 'README' -o -name 'NOTICE' \
               -o -name '*.sh' -o -name '*.py' -o -name '*.pl' \
               -o -name '*.toml' -o -name '*.rs' \) \
            -print0)

if [[ -n "$_LEAK_REPORT" ]]; then
    echo "ABORT: tarball staging tree references docs not bundled in the tarball."
    echo "$_LEAK_REPORT" | sort -u
    echo
    echo "Fix the offending files in dev source, then re-run package-release.sh."
    rm -rf "$STAGING"
    exit 1
fi
echo "✓ no cross-reference leaks in tarball tree"

# ── Signature integrity gate ───────────────────────────────────────────
# v0.11.9: defense-in-depth. Re-verify every signed binary in the
# staging tree right before we tar. Any non-write-intent process that
# happens to strip a PE signature (e.g. `llvm-objcopy --dump-section`,
# which we discovered the hard way mutates its input file even though
# it's nominally read-only) gets caught HERE rather than shipping a
# corrupt tarball.
#
# This was the v0.11.8 mystery: signed binaries became "unsigned with
# SBAT" some time after sign-lamboot.sh ran, and we only discovered
# it post-tarball. With this gate, any future strip-on-read bug fails
# the release loudly at packaging time.
echo
echo "══ Signature integrity gate ══"
sig_failures=0
DB_CERT_ABS="/home/greg/lamboot-dev/keys/db.crt"
for signed in $(find . -name '*-signed.efi' | sort); do
    if sbverify --cert "$DB_CERT_ABS" "$signed" >/dev/null 2>&1; then
        echo "  ✓ $signed"
    else
        echo "  ✗ $signed — SIGNATURE INVALID"
        sig_failures=$((sig_failures + 1))
    fi
done
if (( sig_failures > 0 )); then
    echo
    echo "ABORT: $sig_failures signed binary/binaries failed sbverify in staging."
    echo "       Re-run sign-lamboot.sh and avoid touching dist/EFI/LamBoot/"
    echo "       between sign and re-run of package-release.sh."
    echo "       NEVER use llvm-objcopy --dump-section on a signed binary;"
    echo "       it mutates the input file even when targeting /dev/stdout."
    rm -rf "$STAGING"
    exit 1
fi
echo "✓ all signed binaries verify against $DB_CERT_ABS"

# Checksum manifest for reproducibility
echo "══ Writing manifest ══"
find . -type f ! -name MANIFEST.sha256 | sort | xargs sha256sum > MANIFEST.sha256
echo "✓ MANIFEST.sha256 ($(wc -l < MANIFEST.sha256) files)"

# Tar it
cd "$STAGING"
ABSOLUTE_OUTPUT="/home/greg/lamboot-dev/$OUTPUT_DIR"
mkdir -p "$ABSOLUTE_OUTPUT"
echo
echo "══ Creating tarball ══"
tar czf "${ABSOLUTE_OUTPUT}/${TARBALL_NAME}" "lamboot-${VERSION}/"
ls -la "${ABSOLUTE_OUTPUT}/${TARBALL_NAME}"

# SHA256 — emit with bare filename only (not the full build-host path).
# Otherwise the .sha256 file embeds `/home/<builder>/lamboot-dev/dist/...`
# which (a) leaks the build environment into a public artifact and
# (b) breaks `sha256sum -c` for users whose tarball isn't at that
# absolute path.
(cd "${ABSOLUTE_OUTPUT}" && sha256sum "${TARBALL_NAME}") | tee "${ABSOLUTE_OUTPUT}/${TARBALL_NAME}.sha256"

# Cleanup staging
rm -rf "$STAGING"

echo
echo "✓ Release tarball ready: ${ABSOLUTE_OUTPUT}/${TARBALL_NAME}"
echo "  SHA256: $(awk '{print $1}' ${ABSOLUTE_OUTPUT}/${TARBALL_NAME}.sha256)"
echo
echo "Next steps:"
echo "  1. Tag: git tag -a v${VERSION} -m 'LamBoot v${VERSION}'"
echo "  2. Push tag: git push origin v${VERSION}"
echo "  3. Run the project's publish workflow to stage and ship the release."
