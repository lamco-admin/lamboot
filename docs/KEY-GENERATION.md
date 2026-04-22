# LamBoot Production Signing Key Generation

**Audience:** release engineer preparing LamBoot for public distribution.
**Prerequisites:** OpenSSL 3.x, a Linux workstation with an encrypted home or full-disk encryption, a password manager for passphrase storage, and a secure offline medium (encrypted USB in a safe, or an encrypted archive with a documented custodian) for PK/KEK key material.
**Status:** authoritative procedure for v0.8.3 production keys.

---

## 1. Key hierarchy overview

LamBoot's Secure Boot signing uses a three-tier key hierarchy following UEFI convention:

| Key | Role | Algorithm | Validity | How often used | Storage |
|---|---|---|---|---|---|
| **PK** — Platform Key | Root of trust; signs KEK updates | RSA 4096 / SHA-256 | 10 years | Once at setup, then rarely for rotations | Offline encrypted |
| **KEK** — Key Exchange Key | Signs `db` updates | RSA 4096 / SHA-256 | 10 years | Only during `db` rotation | Offline encrypted |
| **db** — Signature DB | Signs every LamBoot release binary | **RSA 2048** / SHA-256 | 3 years (rotatable) | Every build | Encrypted at rest on signing host |

The algorithm split is deliberate. See [`docs/analysis/RSA-4096-COMPATIBILITY-ANALYSIS-2026-04-20.md`](analysis/RSA-4096-COMPATIBILITY-ANALYSIS-2026-04-20.md) for the full rationale. In short: **the `db` key must be RSA 2048** because shim has an unfixed bug ([Debian #1013320](https://groups.google.com/g/linux.debian.bugs.dist/c/VYecNquj5mk)) that freezes when verifying binaries via an RSA 4096 MOK-enrolled key, and Config 3 (shim + MOK) is LamBoot's default distro-user path. **PK and KEK use RSA 4096** because they operate entirely in firmware variable space and never pass through shim — firmware-level 4096 is universally supported ([Microsoft's UEFI CA 2023](https://support.microsoft.com/en-us/topic/kb5036210-deploying-windows-uefi-ca-2023-certificate-to-secure-boot-allowed-signature-database-db-a68a3eae-292b-4224-9490-299e303b450b) is itself RSA 4096). All keys use SHA-256 for universal spec-mandatory support.

**Never use RSA 4096 for any LamBoot cert that might end up in a MOKList.** That includes the `db` signing key and any intermediate keys derived from it.

---

## 2. Subject Distinguished Name conventions

All production certs use this subject structure, with `CN` varying per key:

```
C=US
ST=IL
O=Lamco Development
OU=LamBoot
CN=<per-key, see below>
emailAddress=office@lamco.io
```

| Key | CN |
|---|---|
| PK  | `LamBoot Platform Key` |
| KEK | `LamBoot Key Exchange Key` |
| db  | `LamBoot Release Signing Key 2026` |

The db `CN` includes the year so that key rotation is self-documenting: the 2029 rotation produces `LamBoot Release Signing Key 2029`, and logs/MOK lists clearly show which generation signed what.

---

## 3. Generation procedure

### 3.1 Set up a working directory

```
cd ~/lamboot-dev
mkdir -p keys-gen
cd keys-gen
```

`keys-gen/` is gitignored (matches the `keys/` prefix rule in `.gitignore`). All generation happens here; only final public certs are copied to `keys/` after verification.

### 3.2 Create the OpenSSL config

Save as `keys-gen/lamboot-key.cnf`:

```
[ req ]
default_bits       = 4096
default_md         = sha256
prompt             = no
encrypt_key        = yes
distinguished_name = req_dn
x509_extensions    = v3_ca

[ req_dn ]
C            = US
ST           = IL
O            = Lamco Development
OU           = LamBoot
CN           = PLACEHOLDER
emailAddress = office@lamco.io

[ v3_ca ]
basicConstraints       = critical, CA:TRUE
keyUsage               = critical, digitalSignature, keyCertSign
subjectKeyIdentifier   = hash
authorityKeyIdentifier = keyid:always
```

### 3.3 Generate the Platform Key (PK)

```
# Copy the template, override CN
sed 's/^CN.*=.*/CN = LamBoot Platform Key/' lamboot-key.cnf > pk.cnf

# Generate RSA 4096 private key — prompts for passphrase
openssl genrsa -aes256 -out PK.key 4096

# Generate self-signed cert valid 10 years
openssl req -new -x509 -sha256 -days 3650 \
    -key PK.key -out PK.crt -config pk.cnf

# Also produce DER form for firmware enrollment
openssl x509 -in PK.crt -outform DER -out PK.der

# Verify
openssl x509 -in PK.crt -noout -subject -issuer -dates -text | grep -E 'Subject:|Issuer:|Not|Public-Key:|Signature Algorithm:' | head -8
```

Expected verification output:
```
Subject: C=US, ST=IL, O=Lamco Development, OU=LamBoot, CN=LamBoot Platform Key/emailAddress=office@lamco.io
Issuer: C=US, ST=IL, O=Lamco Development, OU=LamBoot, CN=LamBoot Platform Key/emailAddress=office@lamco.io
Not Before: <today>
Not After : <today + 10 years>
Signature Algorithm: sha256WithRSAEncryption
        Public-Key: (4096 bit)
```

### 3.4 Generate the Key Exchange Key (KEK)

```
sed 's/^CN.*=.*/CN = LamBoot Key Exchange Key/' lamboot-key.cnf > kek.cnf
openssl genrsa -aes256 -out KEK.key 4096
openssl req -new -x509 -sha256 -days 3650 \
    -key KEK.key -out KEK.crt -config kek.cnf
openssl x509 -in KEK.crt -outform DER -out KEK.der
```

### 3.5 Generate the db signing key (RSA 2048, shorter validity)

```
# Different template — 2048-bit, 3-year validity
cat > db.cnf <<'EOF'
[ req ]
default_bits       = 2048
default_md         = sha256
prompt             = no
encrypt_key        = yes
distinguished_name = req_dn
x509_extensions    = v3_leaf

[ req_dn ]
C            = US
ST           = IL
O            = Lamco Development
OU           = LamBoot
CN           = LamBoot Release Signing Key 2026
emailAddress = office@lamco.io

[ v3_leaf ]
basicConstraints       = critical, CA:FALSE
keyUsage               = critical, digitalSignature
extendedKeyUsage       = codeSigning
subjectKeyIdentifier   = hash
EOF

openssl genrsa -aes256 -out db.key 2048
openssl req -new -x509 -sha256 -days 1095 \
    -key db.key -out db.crt -config db.cnf
openssl x509 -in db.crt -outform DER -out db.der
```

**Note:** db is `CA:FALSE` with `codeSigning` extendedKeyUsage because it signs binaries, not other certs. PK and KEK are `CA:TRUE` because they form a trust chain.

### 3.6 Verify everything

```
for cert in PK KEK db; do
    echo "--- $cert ---"
    openssl x509 -in ${cert}.crt -noout \
        -subject -issuer -dates | head -4
    openssl x509 -in ${cert}.crt -noout -text \
        | grep -E 'Public-Key:|Signature Algorithm:' | head -3
done
```

Expected:
- PK, KEK: `Public-Key: (4096 bit)`, `Signature Algorithm: sha256WithRSAEncryption`
- db: `Public-Key: (2048 bit)`, `Signature Algorithm: sha256WithRSAEncryption`

All three should have `Subject:` containing `C=US, ST=IL, O=Lamco Development, OU=LamBoot`.

### 3.7 Test-sign a binary with the db key

```
# Smoke-test signing works end-to-end
sbsign --key db.key --cert db.crt \
    --output /tmp/test-signed.efi \
    ../dist/EFI/LamBoot/lambootx64.efi

sbverify --cert db.crt /tmp/test-signed.efi
# Expected: Signature verification OK

rm /tmp/test-signed.efi
```

If `sbsign` or `sbverify` are missing: `sudo apt install sbsigntool` (Debian/Ubuntu) or `sudo dnf install sbsigntools` (Fedora).

---

## 4. Storage and handling

### 4.1 db.key — signing host (frequent use)

- Stays on the signing host (build machine / CI signer).
- Encrypted at rest via the `openssl genrsa -aes256` passphrase.
- Passphrase stored in the team password manager under a named entry (e.g., `lamboot-db-signing-key-2026`).
- Never committed to git (protected by `.gitignore` rule added in commit prior).
- Backup: one encrypted copy in the offline medium (§4.2) alongside PK/KEK.
- Long-term: migrate to HSM-backed signing before the 2029 rotation.

### 4.2 PK.key and KEK.key — offline

- Generated once on the signing host, then transferred to offline storage.
- Offline storage options (pick one, document which):
  - Encrypted USB stick in a physical safe (practical)
  - LUKS-encrypted archive on detachable storage (practical)
  - HSM-backed keys (ideal long-term, out of scope for v0.8.3)
- After transfer, shred local copies:
  ```
  shred -u PK.key KEK.key   # passphrase-protected, but shred anyway
  ```
- Passphrases stored in password manager under distinct entries, never co-located with key material.
- Document the custodian and location in a sealed access doc (not in this repo).

### 4.3 Public certs — distributed

- `PK.crt`, `PK.der`, `KEK.crt`, `KEK.der`, `db.crt`, `db.der` are public by construction.
- Copy these into the project's `keys/` directory (gitignored by default; use `git add -f` only for intentional distribution — preferred path is to ship them via `dist/` release artifacts).
- Ship `db.der` in the release tarball so installers can enroll it into MOK.
- Ship `db.der` embedded in `OVMF_VARS_lamboot.fd` for Config 4 zero-touch deployment.

---

## 5. Retiring the LamBoot Dev keys

The existing `keys/*.{crt,der,key}` material has `CN=LamBoot Dev PK/KEK/db` — labeled dev by design. Those keys:

- Were used to sign drivers and modules shipped in `dist/EFI/LamBoot/drivers/` and `dist/EFI/LamBoot/modules/` during development.
- Were never distributed to external users.
- Must not appear in any production release.

Retirement procedure:

1. Copy current `keys/` to `keys-archive/dev-keys-2026-04-retired/` (still gitignored) with a `RETIRED.md` note stating date and reason.
2. Replace `keys/` contents with production PK/KEK/db generated per §3.
3. Re-sign every artifact in `dist/` using production db.key (`./build.sh` after updating `build.sh` per Task #4).
4. Delete `keys-archive/` after successful production build + testing (retain for 90 days as rollback insurance, then shred).

---

## 6. Rotation plan

### 6.1 db rotation — every 3 years (planned 2029)

Well ahead of expiry (~3 months before):

1. Generate new db keypair following §3.5 with `CN=LamBoot Release Signing Key 2029`.
2. Cross-sign: new db cert signed with KEK (append to `db` variable alongside old cert — both trusted during transition).
3. Ship updated release tarball signed with new db; installers enroll both old and new certs.
4. Publish rotation notice on project website + release notes with MOK re-enrollment instructions for existing users.
5. 12 months after issuance of new db: remove old db from firmware/MOK enrollment artifacts (revocation via KEK-signed db-removal EFI Signature List).

### 6.2 PK/KEK rotation — 10-year cadence (planned 2036)

Rare. Procedure documented at rotation time; requires coordinated firmware updates for Config 4 deployments.

### 6.3 Compromise response

If `db.key` is known or suspected leaked:

1. Immediately stop signing new releases.
2. Generate new db keypair per §3.5 (emergency rotation).
3. Sign a `dbx` (forbidden signature database) entry revoking the compromised cert, signed with KEK.
4. Ship emergency release with revocation + new signing key.
5. Public advisory with CVE-equivalent severity note.

If `KEK.key` or `PK.key` is compromised: requires firmware-level remediation. Same steps plus update to `OVMF_VARS_lamboot.fd` and public advisory requiring operators to re-enroll.

---

## 7. CI / automated signing considerations

For future CI signing (out of scope for v0.8.3 initial release, but plan):

- `db.key` passphrase injected via CI secret store (GitHub Actions encrypted secrets, HashiCorp Vault, etc.).
- Signing happens in an isolated CI runner; key decrypted into tmpfs, never touches persistent disk.
- Signed artifact output uploaded to release assets; key is zeroed from tmpfs at job end.
- Consider HSM-backed signing (YubiHSM 2, cloud KMS) for stronger key isolation.

---

## 8. Quick reference: commands from scratch

For a release engineer setting up a new signing host:

```
cd ~/lamboot-dev
mkdir -p keys-gen && cd keys-gen
# (paste the OpenSSL config from §3.2 into lamboot-key.cnf)

openssl genrsa -aes256 -out PK.key 4096
sed 's/^CN.*=.*/CN = LamBoot Platform Key/' lamboot-key.cnf > pk.cnf
openssl req -new -x509 -sha256 -days 3650 -key PK.key -out PK.crt -config pk.cnf
openssl x509 -in PK.crt -outform DER -out PK.der

openssl genrsa -aes256 -out KEK.key 4096
sed 's/^CN.*=.*/CN = LamBoot Key Exchange Key/' lamboot-key.cnf > kek.cnf
openssl req -new -x509 -sha256 -days 3650 -key KEK.key -out KEK.crt -config kek.cnf
openssl x509 -in KEK.crt -outform DER -out KEK.der

# db uses its own config (RSA 2048, codeSigning EKU, 3-year validity)
# (paste db.cnf from §3.5)
openssl genrsa -aes256 -out db.key 2048
openssl req -new -x509 -sha256 -days 1095 -key db.key -out db.crt -config db.cnf
openssl x509 -in db.crt -outform DER -out db.der

# Verify all three
for c in PK KEK db; do echo "--- $c ---"; openssl x509 -in ${c}.crt -noout -subject -dates; done

# Move public certs into keys/
cp *.crt *.der ../keys/
# Move private keys to offline storage (PK/KEK) and signing host (db)
# Shred local generation directory
cd ..
shred -u keys-gen/PK.key keys-gen/KEK.key
rm -rf keys-gen   # db.key has already been moved to signing host location
```

---

## 9. References

- [UEFI Specification 2.11 §32 — Secure Boot and Driver Signing](https://uefi.org/specs/UEFI/2.11/32_Secure_Boot_and_Driver_Signing.html)
- [Microsoft — Windows Secure Boot Key Creation and Management Guidance](https://learn.microsoft.com/en-us/windows-hardware/manufacture/desktop/windows-secure-boot-key-creation-and-management-guidance)
- [Debian Bug #1013320 — the RSA 4096 MOK constraint](https://groups.google.com/g/linux.debian.bugs.dist/c/VYecNquj5mk)
- [`docs/analysis/RSA-4096-COMPATIBILITY-ANALYSIS-2026-04-20.md`](analysis/RSA-4096-COMPATIBILITY-ANALYSIS-2026-04-20.md) — the research behind the algorithm split
- [`docs/SECURE-BOOT-DEPLOYMENT.md`](SECURE-BOOT-DEPLOYMENT.md) — deployment guide consuming these keys
