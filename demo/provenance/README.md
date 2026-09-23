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

## Verify an exported output

With an exported labelled text and a PEM bundle of certificates you explicitly
trust, run:

```sh
commonmeasure provenance output.txt --trust-anchors trusted-roots.pem --require-trusted
```

The files in this command are operator inputs. Verification reads them locally,
records the bundle's hash and checks claim and CAWG identity trust. It needs no
Hub, signing key, provider account or originating run directory. It makes no
network calls and therefore does not check current online revocation status.
Without the flags, valid but untrusted credentials are still inspectable. Invalid
credentials fail the command in both modes, with the SDK result in JSON where
the credential can be parsed.

The CLI fixture test explicitly trusts the demonstration root to exercise this
path, then checks an unrelated root and altered text. This is fixture-tested
local trust, not C2PA trust-list membership or live Encypher verification. The
demonstration private key is public and must never establish production trust.

## Encypher batch signing

Status: experimental, fixture-tested adapter; live interoperability is incomplete.
Ingredient-schema compatibility, readable exported metadata and complete training-use
declarations are required before it can publish a credential.
An Encypher account and an operator-chosen trust bundle are prerequisites.

Add this field beside the existing `training_mining` declaration in a job's
`output_provenance` object:

```json
"encypher": { "send_answer_and_source_record": true }
```

This permits uploading every answered plan's answer and its source references,
hashes, grades, run identifiers and output-use preferences to Encypher. Answers
may contain source excerpts. The adapter requests no attribution indexing or
manifest database storage; complete service retention remains unverified.

Configure the operator environment with real local paths:

```sh
export COMMONMEASURE_PROVENANCE_SIGNER=encypher
export ENCYPHER_API_KEY_FILE="/path/to/private/encypher-key"
export COMMONMEASURE_PROVENANCE_TRUST_ANCHORS="/path/to/trusted-roots.pem"
```

The key file contains only the API key, optionally followed by a newline, and
should be readable only by its operator. `ENCYPHER_API_KEY` is an alternative;
setting both is refused. No key belongs in the job, source policy or repository.
Choosing `COMMONMEASURE_PROVENANCE_SIGNER=local` uses the existing certificate
and key route. A job requesting Encypher never falls back to local signing.

Returned text must pass local integrity, trust and assertion checks before it is
published. A provider error, unexpected coercion, changed answer or changed
source evidence produces an unavailable invocation. Signing cost remains unknown.
There is one request per answered plan, no automatic retry, and no redirect.
The provider signs the claim; its certificate alone does not establish operator
identity. The generic external processor protocol and Hub receipts remain planned.

## Synthetic live trial

The following integration check has not passed live acceptance. It uses
only synthetic text and source references, calls the real signing endpoint and
may consume account quota. With the authorised key and trust bundle configured:

```sh
COMMONMEASURE_ENCYPHER_LIVE_OUTPUT=/tmp/commonmeasure-encypher-live \
  cargo test --locked --offline -p commonmeasure-runtime --lib \
  processor::provenance::encypher::tests::live_encypher_signs_synthetic_content \
  -- --ignored --exact
```

Each attempt creates its own synthetic run directory containing `receipt.json`
and the synthetic `request.json` when a request is prepared. A successful attempt
also saves the checked `response.json`, the served response bytes for hash
comparison, `output.txt` and `output.c2pa`. Failed responses are not captured by
the test. No credential is
published when the independent checks fail. A lost response may follow completed
signing: inspect the receipt and account before choosing to repeat the trial.
Verify a successful exported text separately with the command above and the same
trust bundle. The trial establishes provider interoperability only; it does not
establish real publisher acquisition or the full agent lifecycle.
