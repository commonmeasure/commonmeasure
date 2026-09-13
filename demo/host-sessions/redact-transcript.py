#!/usr/bin/env python3
"""Redact a Claude Code transcript into the fixture the snapshot reader is
tested against: every record keeps its shape, its kinds, its identifiers,
its counters and the length of every text, and none of the text.

Usage: redact-transcript.py <transcript.jsonl> > <fixture.jsonl>

A string value is kept verbatim only under the keys that name a kind, an
identifier, a tool, a skill, an agent type, a model, a timestamp or a flag;
every other string is replaced by a run of the same number of characters,
so a reader that measures text lengths sees what it saw on the original.
Lines that are not JSON are dropped. Local paths are strings like any other
and are replaced.
"""

import json
import sys

KEEP = {
    "type",
    "subtype",
    "role",
    "name",
    "tool_name",
    "toolName",
    "id",
    "tool_use_id",
    "toolUseID",
    "uuid",
    "parentUuid",
    "sessionId",
    "session_id",
    "requestId",
    "timestamp",
    "model",
    "modelId",
    "version",
    "hookEvent",
    "hookName",
    "names",
    "addedNames",
    "removedNames",
    "readdedNames",
    "wireHiddenNames",
    "addedTypes",
    "removedTypes",
    "stop_reason",
    "entrypoint",
    "userType",
    "permissionMode",
    "level",
    "operation",
    "commandMode",
    "kind",
}

FILLER = "x"


def redact(value, key=None):
    if isinstance(value, dict):
        return {k: redact(v, k) for k, v in value.items()}
    if isinstance(value, list):
        # A list under a kept key is a list of names.
        if key in KEEP:
            return [v if isinstance(v, str) else redact(v) for v in value]
        return [redact(v) for v in value]
    if isinstance(value, str):
        if key in KEEP:
            return value
        return FILLER * len(value)
    return value


def main(path):
    with open(path, encoding="utf-8") as transcript:
        for line in transcript:
            try:
                record = json.loads(line)
            except json.JSONDecodeError:
                continue
            sys.stdout.write(json.dumps(redact(record), ensure_ascii=False, separators=(",", ":")))
            sys.stdout.write("\n")


if __name__ == "__main__":
    main(sys.argv[1])
