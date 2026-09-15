# Describe transcript import accurately

## Package

WP-27 and WP-39: session evidence and host support grades.

## Goal

The import capability list distinguishes unsupported importers from an
absence of observable source records in a host's transcripts.

## Files

- `crates/commonmeasure-harness/src/import.rs`
- `docs/contracts/session-evidence.md`
- `crates/commonmeasure-cli/tests/recorded_sessions.rs`

## Done when

Check completed Codex web-search records and Pi extraction results against
redacted recorded host sessions. The current import reasons assert Codex
only shells out and Pi has no observable crossings; those explanations
must not stand in for a capability test. State precisely which record
shapes can be reconstructed and which importer is unavailable.

Correct the capability reasons and corresponding assertions. Any new
import path needs a recorded session driven through the real parser,
reconstructed grading and explicit limits on what the transcript proves.
Missing evidence stays unavailable. Preserve the rule that reconstructed
crossings do not become witnessed telemetry. Run affected integration
and documentation checks. Delete this brief on landing.

## Out of scope

No transcript collection from other users, change to telemetry privacy or
promotion of reconstructed evidence to mediated evidence.
