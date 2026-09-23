#!/usr/bin/env sh
# Generates the demonstration signing identity for output provenance labels:
# a private root, and an end-entity certificate it issues, with the key usage
# and extended key usage the C2PA certificate profile requires. The root is on
# no trust list, so every label signed with this identity validates as valid
# but untrusted, which is the state the record has to name.
#
# Run from this directory. Overwrites signer.pem and signer-key.pem.
set -eu
cd "$(dirname "$0")"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes \
  -keyout "$work/root-key.pem" -out "$work/root.pem" -days 3650 \
  -subj "/CN=Common Measure demonstration root/O=Common Measure Ltd" \
  -addext "basicConstraints=critical,CA:TRUE" \
  -addext "keyUsage=critical,keyCertSign,cRLSign" 2>/dev/null
openssl req -new -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes \
  -keyout "$work/signer-key.pem" -out "$work/signer.csr" \
  -subj "/CN=Common Measure demonstration signer/O=Common Measure Ltd" 2>/dev/null
printf '%s\n' \
  "basicConstraints=critical,CA:FALSE" \
  "keyUsage=critical,digitalSignature" \
  "extendedKeyUsage=emailProtection" \
  "subjectKeyIdentifier=hash" \
  "authorityKeyIdentifier=keyid" > "$work/signer.ext"
openssl x509 -req -in "$work/signer.csr" -CA "$work/root.pem" -CAkey "$work/root-key.pem" \
  -CAcreateserial -out "$work/signer-cert.pem" -days 3650 -extfile "$work/signer.ext" 2>/dev/null

# The chain the SDK expects: the end-entity certificate first, then its issuer.
cat "$work/signer-cert.pem" "$work/root.pem" > signer.pem
# The key in PKCS#8, the one encoding the SDK reads.
openssl pkcs8 -topk8 -nocrypt -in "$work/signer-key.pem" -out signer-key.pem
echo "wrote signer.pem and signer-key.pem"
