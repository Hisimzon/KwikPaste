#!/usr/bin/env bash
# Generates the self-signed code-signing certificate macOS builds are signed with. There is no Apple
# Developer account, and an ad-hoc signature's designated requirement is the cdhash, which changes with
# every version, so macOS drops the Accessibility grant on each update. Signed with one fixed certificate,
# the requirement is identifier + certificate and stays the same across versions.
#
# The owner runs this once (Git Bash on Windows works) and stores the results as repository secrets:
#   MACOS_SIGNING_CERTIFICATE           contents of <out-dir>/kwikpaste-macos.p12.base64
#   MACOS_SIGNING_CERTIFICATE_PASSWORD  contents of <out-dir>/password.txt
# then deletes <out-dir>. Replacing the certificate later makes every user grant Accessibility again.
# native-release.yml dry runs call this for a throwaway certificate.
#
# Usage: macos-signing-cert.sh <out-dir>
set -euo pipefail

out="$1"
mkdir -p "${out}"
cd "${out}"

export P12_PASSWORD="$(openssl rand -hex 24)"
cat > cert.cnf <<'EOF'
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = KwikPaste Code Signing
O = FastThree
[ext]
basicConstraints = critical, CA:FALSE
keyUsage = critical, digitalSignature
extendedKeyUsage = critical, codeSigning
subjectKeyIdentifier = hash
EOF

openssl req -x509 -newkey rsa:2048 -nodes -days 10950 -config cert.cnf -keyout key.pem -out cert.pem 2> /dev/null
# 3DES + SHA-1 MAC: macOS `security import` rejects OpenSSL 3's default AES/PBKDF2 PKCS#12.
openssl pkcs12 -export -inkey key.pem -in cert.pem -name "KwikPaste Code Signing" \
  -keypbe PBE-SHA1-3DES -certpbe PBE-SHA1-3DES -macalg sha1 \
  -passout env:P12_PASSWORD -out kwikpaste-macos.p12
openssl base64 -A -in kwikpaste-macos.p12 -out kwikpaste-macos.p12.base64
printf '%s' "${P12_PASSWORD}" > password.txt
rm key.pem cert.cnf
