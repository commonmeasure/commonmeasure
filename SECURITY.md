# Security

Report a security problem in Common Measure to **hello@commonmeasure.ai**.
Do not open a public issue for it.

## What to send

- The version (`commonmeasure --version`) or the commit, and the platform.
- What the problem lets an attacker do, and the steps that show it. A
  session log, a run directory or a policy file that reproduces it is more
  useful than a description.
- Whether you have told anyone else.

Encrypt the report if it contains a live credential or licensed content;
otherwise plain mail is fine. Never include a working credential in a report
when a redacted one shows the problem.

## What happens next

- You get an acknowledgement within three working days.
- We confirm or dispute the problem, agree a disclosure date with you, and
  keep you informed until a fix is released. The default is ninety days from
  the report, sooner where the fix is ready.
- A fixed problem is credited to you in the release notes unless you ask
  otherwise.

## What is in scope

- The `commonmeasure` binary and the crates in this repository.
- The harness plugin under `plugin/` and the installer `install.sh`.
- The operator console served by `commonmeasure serve`.
- The evidence and policy files under the operator home
  (`~/.commonmeasure/` by default): anything that lets content, a credential
  or a record leave the machine other than through a configured receiver, or
  that lets a source be admitted against operator policy, is in scope.

Third-party services the product talks to (content suppliers, an inference
gateway, a telemetry receiver) have their own disclosure routes.

## Supported versions

Fixes are released for the latest minor version. Older releases are not
patched; update to the current release.
