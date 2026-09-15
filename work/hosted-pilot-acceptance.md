# Hosted pilot acceptance

Current delivery status remains in ROADMAP.md and the Hub board. Local
implementation was integrated at the 15 September checkpoint; this brief
retains the outstanding hosted walkthrough and links to its existing evidence.

## Goal and acceptance

Complete the first-user path of WP-45 and the hosted managed-policy evidence
of WP-19: a fresh test home, explicit directory policy published before managed
connection, admitted and refused mediated fetches, an accurate local record,
and a batch delivered into the intended hosted organisation. Exercise signed
rollout, rollback refusal and offline enforcement through real seams. Record
tested revisions and redacted evidence. Operator feedback remains a human
acceptance item and must not be invented from an automated run.

## Existing evidence

The released 0.3.2 edge and a real local Hub demonstrated selected-directory
delivery, withholding an unselected session, rollout to revision 2, refusal
of the captured revision-1 envelope through loopback replay and enforcement
with the management endpoint unavailable. The repeatable driver and recorded
run are `demo/enrolment/local-managed.py` and
`demo/enrolment/local-managed-run.txt`.

The checkpoint also integrated policy-first onboarding, invited pilot teams,
the general-purpose template and directory enrolment with independent signed
reporting grants. `work/directory-enrolment-handoff.md` records the local
CLI/Hub integration evidence. WP-41 records the completed console integration;
WP-44 and the Hub board own the operator screens and their remaining work.

These local runs do not establish hosted acceptance or interactive invocation
through an agent host. The operations repository's
`runbooks/checkpoint-2026-09-15.md` records the integrated validation snapshot;
its `runbooks/deploy-hub-cloud.md` owns running revisions and deployment
evidence. Publish and verify the product website's guide destinations before
including the Hub documentation redirects in a deployment.
