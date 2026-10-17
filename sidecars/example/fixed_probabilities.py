# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Protocol example only: deterministic hash-based probabilities, no model."""

import hashlib
import json
import sys

print(json.dumps({"hello": {
    "model": "fixed-probabilities/1", "family": "example", "device": "cpu",
    "questions": ["supported", "contradicts"],
}}), flush=True)
requests = 0
for line in sys.stdin:
    case = json.loads(line)
    # This deliberately does not judge support. It exercises both display
    # outcomes reproducibly without shipping an evaluation model.
    supported = hashlib.sha256(case["summary"].encode()).digest()[0] < 128
    print(json.dumps({
        "id": case["id"],
        "p_supported": 0.99 if supported else 0.01,
        "p_contradicts": 0.01 if supported else 0.99,
        "latency_ms": 0,
    }), flush=True)
    requests += 1
print(json.dumps({"bye": {"requests": requests}}), flush=True)
