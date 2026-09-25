---
name: commonmeasure-enrol
description: Enrol, inspect or remove Common Measure reporting for the current project directory when explicitly requested.
---

Set up the actual current agent project using the shared Common Measure binary.
Resolve and show the directory first. If the project is missing or ambiguous,
ask which directory to use. Never treat `/` as a project.

Call `context_status` and compare its `cwd` with the host project. Use
`context_enrol` with that explicit directory for status and changes when they
agree. If they disagree, use the CLI to configure the directory and tell the
user to restart the MCP server in that project before claiming its retrievals
are governed by this setup.

Show the existing edge, organisation, applied policy revision and reporting
status. Reuse enrolment and keys. If disconnected, guide
`commonmeasure connect <named-hub> --token <token> --managed` using the user's
named hub and existing secure token flow; never print or request keys in chat.

Ask for the project name and local recording versus hub reporting only when
not already specified. Before hub reporting, explain that coverage is the
canonical root and descendants, including existing eligible witnessed evidence;
related Git worktrees need separate enrolment. Existing policy restrictions,
confidential exclusions and private/internal evidence remain enforced.

CLI equivalents (quote the actual path and name safely):

- `commonmeasure enrol --directory <path>`: status.
- `commonmeasure enrol --directory <path> --name <name> --reporting local`.
- `commonmeasure enrol --directory <path> --name <name> --reporting hub --include-history`.
- `commonmeasure enrol --directory <path> --sync`: refresh applied policy and reporting approvals after owner approval.
- `commonmeasure enrol --directory <path> --remove`: stop local reporting.

Use the installed Common Measure binary. A managed pending request needs owner
approval on the connected Hub's `/directories` page; a false winning policy
scope still withholds reporting after approval. Report the actual returned
state. No operation grants additional host permissions.

Verify status, then a user-authorised permitted retrieval and
`commonmeasure relay --dry-run`; run `commonmeasure relay` when reporting is
authorised. Report actual delivered totals separately from local evidence and
zero eligible events. MCP prompts are available only if the host exposes them;
do not claim every MCP client provides a slash command.
