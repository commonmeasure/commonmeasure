#!/usr/bin/env python3
"""Normalise the pinned upstream CSV without installing upstream dependencies."""
import argparse
import csv
import hashlib
import io
import json
from pathlib import Path

COMMIT = "99feb7decc9be67edb49a63d3985cf6094c873f2"
SHA256 = "feee3f7e7db3617e94e8fcf1977b756ec420ef8568f4e0fcbbe0e92e9d5fc032"


def convert(raw):
    if hashlib.sha256(raw).hexdigest() != SHA256:
        raise ValueError("CSV digest differs from the pinned upstream dataset")
    reader = csv.DictReader(io.StringIO(raw.decode("utf-8"), newline=""))
    if reader.fieldnames != ["metadata", "problem", "answer"]:
        raise ValueError("Unexpected SimpleQA columns")
    rows = list(reader)
    if len(rows) != 4326:
        raise ValueError("Expected 4,326 SimpleQA cases")
    return {
        "schema": "contextops-simpleqa-dataset/v1",
        "source_commit": COMMIT,
        "source_sha256": "sha256:" + SHA256,
        "source_rows": len(rows),
        "cases": [
            {"id": f"simpleqa-{i:04}", "question": row["problem"],
             "reference_answer": row["answer"]}
            for i, row in enumerate(rows, 1)
        ],
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("csv", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    dataset = convert(args.csv.read_bytes())
    with args.output.open("x", encoding="utf-8") as output:
        json.dump(dataset, output, ensure_ascii=False, indent=2)
        output.write("\n")
    print(f"Wrote {len(dataset['cases'])} cases to {args.output}")


if __name__ == "__main__":
    main()
