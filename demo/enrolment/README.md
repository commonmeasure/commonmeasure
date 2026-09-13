# Enrolment against a real hub

`real-hub-run.txt` is the transcript of the ignored live test
`a_real_hub_enrols_revokes_and_disconnects_this_edge` in
`crates/commonmeasure-cli/tests/connect_e2e.rs`, run against a
Common Measure Hub server on this machine: the owner mints a token
through the hub's API, this binary connects, a session names the key id,
a cleared crossing relays, the owner revokes the key at the hub, the next
relay run learns it, the next session names the revoked key, and the edge
disconnects. The test's doc comment says how to run it; every secret in
the transcript is redacted by the test before it is printed.

To regenerate, with the hub running and the two environment variables the
doc comment names exported:

```sh
cargo test -p commonmeasure-cli --test connect_e2e -- --ignored --nocapture \
  a_real_hub_enrols_revokes_and_disconnects_this_edge 2>/dev/null \
  | sed -n '/^## 1\./,/^test result/p' | grep -v '^test ' > demo/enrolment/real-hub-run.txt
```

## The Ozone pilot path

`ozone-pilot-run.txt` is the transcript of the pilot path run by hand
against a Common Measure Hub server on this machine: an owner mints a
token, this binary connects from an empty home, a policy scope clears the
working directory's engagement for egress, one live Ozone Live search is
made through the mediated search tool, the relay delivers the three
retrieval events with `commonmeasure-supplier: ozone`, the hub imports the
three publishers as the network `ozone` with one shared consumer, resolves
each `www.` host to its parent registration, delivers every event onward
to the consumer (the same hub under a second organisation's ingest key),
and answers the network report filtered to that supplier. The passages
Ozone returned are licensed publisher content and are withheld from the
transcript; every secret and machine path is redacted. Two departures
from the documented path are stated in the transcript where they happen:
Ozone's ownership map answered 503, so the import body was built from the
hosts the search returned; and the stored destination was redirected to
the loopback consumer by SQL, because the API accepts `https` only.
