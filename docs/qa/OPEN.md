---
title: Open findings
draft: true
---

# Open findings

The QA gate ([`gate.md`](gate.md)) is a read-only pass over the tree after each work
package and before a release checkpoint; each finding carries exact evidence,
the consequence, the simpler correction and what closes it, with a verdict of
PASS or BLOCK. This file is the one home for every open finding. A finding
leaves it only with a closure commit or a stated retirement reason; closed
findings are not kept here, git holds the closure commits.

No finding is open. A gate that opens one adds its row to the table below.

| ID | Sev | Finding | Where | Closes when |
|---|---|---|---|---|
