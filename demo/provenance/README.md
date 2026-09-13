# The demonstration signing identity

Output provenance labels (`docs/contracts/processor.md`, `output-provenance`)
are signed with a certificate the operator supplies. This directory holds a
demonstration identity for the example run and the tests: a private root
and the end-entity certificate it issued, with the key usage the C2PA
certificate profile requires. It is committed, private key included; it
authorises nothing and is on no trust list, so a label signed with it
validates as valid but untrusted.

- `signer.pem`: the certificate chain, end-entity certificate first.
- `signer-key.pem`: the matching PKCS#8 private key (ECDSA P-256).
- `generate.sh`: the `openssl` commands that produced both.

An operator names its own identity in the environment of the batch runner:

- `COMMONMEASURE_PROVENANCE_CERTIFICATE`: the certificate chain PEM;
- `COMMONMEASURE_PROVENANCE_KEY`: the PKCS#8 key PEM.

With neither set, a suite that declares `output_provenance` records an
unavailable invocation naming both variables on every answered plan and
leaves the answer unlabelled. With one set and not the other, or a file that
cannot be read, the record names the file and the reason.

To regenerate the demonstration identity:

```sh
sh demo/provenance/generate.sh
```

The `provenance-example` recipe in the `justfile` produces a labelled run
under this identity from the replay recordings. It needs an inference
gateway, because a plan without an answer has nothing to label, and the run
it produces is not committed.
