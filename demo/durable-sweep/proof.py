#!/usr/bin/env python3
"""Local replay-only queue experiment, not a hosted scheduler or billing ledger."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import uuid

HERE = Path(__file__).resolve().parent


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()


def digest(data):
    return hashlib.sha256(data).hexdigest()


def atomic_json(path, value):
    temporary = path.with_suffix(".tmp")
    with temporary.open("xb") as stream:
        stream.write(canonical(value))
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temporary, path)
    fd = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


class Queue:
    """Trusted local operator API. Scope checks are not authentication."""

    def __init__(self, root):
        self.root = Path(root).resolve()
        self.root.mkdir(parents=True, exist_ok=True, mode=0o700)
        self.db = sqlite3.connect(self.root / "queue.sqlite", timeout=10, isolation_level=None)
        self.db.row_factory = sqlite3.Row
        self.db.execute("PRAGMA journal_mode=WAL")
        self.db.execute("PRAGMA synchronous=FULL")
        self.db.executescript("""
        CREATE TABLE IF NOT EXISTS projects (
          org TEXT, project TEXT, allowance INTEGER NOT NULL CHECK(allowance >= 0),
          enabled INTEGER NOT NULL DEFAULT 1, PRIMARY KEY(org, project));
        CREATE TABLE IF NOT EXISTS executions (
          id TEXT PRIMARY KEY, org TEXT, project TEXT, slot TEXT, definition TEXT,
          version TEXT, state TEXT NOT NULL DEFAULT 'queued', token INTEGER NOT NULL DEFAULT 0,
          held INTEGER NOT NULL DEFAULT 0, export_hash TEXT,
          UNIQUE(org, project, slot));
        """)

    def close(self):
        self.db.close()

    def transaction(self, operation):
        self.db.execute("BEGIN IMMEDIATE")
        try:
            result = operation()
            self.db.execute("COMMIT")
            return result
        except BaseException:
            self.db.execute("ROLLBACK")
            raise

    def project(self, scope, allowance):
        if type(allowance) is not int or allowance < 0:
            raise ValueError("allowance must be non-negative integer replay call units")
        self.db.execute("INSERT INTO projects(org,project,allowance) VALUES(?,?,?)", (*scope, allowance))

    def revoke(self, scope):
        self.db.execute("UPDATE projects SET enabled=0 WHERE org=? AND project=?", scope)

    def authorised(self, scope):
        row = self.db.execute("SELECT * FROM projects WHERE org=? AND project=?", scope).fetchone()
        if row is None or not row["enabled"]:
            raise PermissionError("project unavailable or revoked")
        return row

    def get(self, scope, execution):
        self.authorised(scope)
        row = self.db.execute("SELECT * FROM executions WHERE id=? AND org=? AND project=?",
                              (execution, *scope)).fetchone()
        if row is None:
            raise PermissionError("execution outside project")
        return row

    def submit(self, scope, slot, definition):
        if definition.get("mode") != "retrieval-replay" or definition.get("money_ceiling") is not None:
            raise ValueError("only replay call limits supported; no enforceable monetary bound")
        if not definition.get("cases") or definition.get("providers") != ["exa"]:
            raise ValueError("proof supports a non-empty synthetic dataset and Exa replay only")
        ids = [case["id"] for case in definition["cases"]]
        if len(set(ids)) != len(ids) or not all(isinstance(value, str) and value for value in ids):
            raise ValueError("case IDs must be unique non-empty strings")
        body = canonical(definition).decode()
        version = digest(body.encode())

        def insert():
            self.authorised(scope)
            prior = self.db.execute("SELECT * FROM executions WHERE org=? AND project=? AND slot=?",
                                    (*scope, slot)).fetchone()
            if prior:
                if prior["version"] != version:
                    raise ValueError("slot already sealed with another definition")
                return prior["id"]
            execution = str(uuid.uuid4())
            self.db.execute("INSERT INTO executions(id,org,project,slot,definition,version) VALUES(?,?,?,?,?,?)",
                            (execution, *scope, slot, body, version))
            return execution
        return self.transaction(insert)

    def claim(self, scope, execution):
        def reserve():
            row = self.get(scope, execution)
            if row["state"] != "queued":
                raise ValueError("not queued; interrupted work is never automatically retried")
            count = len(json.loads(row["definition"])["cases"])
            held = self.db.execute("SELECT COALESCE(SUM(held),0) FROM executions WHERE org=? AND project=?",
                                   scope).fetchone()[0]
            if held + count > self.authorised(scope)["allowance"]:
                raise ValueError("replay call allowance exhausted")
            token = row["token"] + 1
            self.db.execute("UPDATE executions SET state='reserved',token=?,held=? WHERE id=?",
                            (token, count, execution))
            return token
        return self.transaction(reserve)

    def transition(self, scope, execution, token, previous, next_state, export_hash=None):
        def update():
            row = self.get(scope, execution)
            if row["token"] != token or row["state"] != previous:
                raise ValueError("stale worker or invalid disposition")
            self.db.execute("UPDATE executions SET state=?,export_hash=? WHERE id=?",
                            (next_state, export_hash, execution))
        self.transaction(update)

    def recover(self, scope, execution):
        """Explicit operator recovery. Possible spend never expires or gets resent."""
        def update():
            row = self.get(scope, execution)
            if row["state"] == "reserved":
                # No child has been launched at this persisted boundary.
                self.db.execute("UPDATE executions SET state='not_sent',held=0,token=token+1 WHERE id=?",
                                (execution,))
            elif row["state"] == "dispatched":
                self.db.execute("UPDATE executions SET state='outcome_unknown',token=token+1 WHERE id=?",
                                (execution,))
            return self.get(scope, execution)["state"]
        return self.transaction(update)

    def export(self, scope, execution):
        row = self.get(scope, execution)
        if row["state"] not in ("completed", "partial"):
            raise ValueError("no indexed export")
        body = (self.root / execution / "export.json").read_bytes()
        if digest(body) != row["export_hash"]:
            raise ValueError("export integrity failure")
        result = json.loads(body)
        validate_export(result)
        return result


def fixture(directory):
    """Synthetic provider-shaped bytes: no real supplier call or licensed capture."""
    directory.mkdir()
    body = {"results": [{"id": "https://example.invalid/synthetic", "url": "https://example.invalid/synthetic",
                         "title": "Synthetic catalogue", "text": "Cobalt makes synthetic blue widgets."}]}
    endpoint = "https://api.exa.ai/search"
    atomic_json(directory / "capture.json", {"request": {"url": endpoint},
                                            "response": {"status": 200, "body": body}})
    atomic_json(directory / "replay-manifest.json", {
        "manifest_version": "contextops-replay/v1", "derived_from": "synthetic public-safe test input",
        "recordings": [{"provider": "exa", "capability": "search", "source": "capture.json",
                        "captured_at": "2026-01-01T00:00:00Z", "recorded_endpoint": endpoint,
                        "response_sha256": "sha256:" + digest(canonical(body)), "redactions": "none; synthetic",
                        "permitted_use": "local workflow testing; not supplier performance evidence"}]})


def definition(use_case):
    if use_case not in ("agency", "supplier"):
        raise ValueError("unknown configuration")
    return {"mode": "retrieval-replay", "use_case": use_case, "providers": ["exa"],
            "cases": [{"id": "case-1", "question": "Who makes blue widgets?"},
                      {"id": "case-2", "question": "What does Cobalt make?"}],
            "result_limit": 1, "context_words": 64, "constraints": [],
            "measurement_basis": "synthetic loopback; not supplier timing", "money_ceiling": None}


def suite(config, case):
    return {"suite_version": "durable-sweep-example/v1", "label": "Synthetic retrieval replay",
            "job": {"id": str(uuid.uuid4()), "kind": "research.answer", "prompt": case["question"],
                    "policy_mode": "strict", "objective": {"kind": "minimise_cost"},
                    "constraints": [{"kind": "maximum_context_tokens", "tokens": config["context_words"]}]
                                   + config["constraints"], "evidence_requirements": []},
            "model_plan": {"name": "unconfigured", "version": "1", "model": "not-invoked"},
            "providers": config["providers"], "result_limit": config["result_limit"]}


def measurement(value, unit, basis, reason=None):
    return {"value": value, "unit": unit, "basis": basis,
            "missing_reason": reason if value is None else None}


def validate_export(result):
    """Validate this example's narrow metrics-only shape, not a global wire contract."""
    if set(result) != {"schema_version", "execution", "version", "producer", "state", "selected", "measured", "cells"}:
        raise ValueError("unexpected export fields")
    if result["schema_version"] != "contextops-local-sweep/v1":
        raise ValueError("unsupported example export")
    uuid.UUID(result["execution"])
    if set(result["producer"]) != {"binary_sha256", "wrapper_sha256"}:
        raise ValueError("invalid producer")
    for value in (result["version"], *result["producer"].values()):
        if len(value) != 64 or any(character not in "0123456789abcdef" for character in value):
            raise ValueError("invalid digest")
    if type(result["selected"]) is not int or result["selected"] < 1:
        raise ValueError("invalid selected denominator")
    if result["selected"] != len(result["cells"]):
        raise ValueError("selected denominator lost")
    if result["measured"] != sum(cell["outcome"] == "retrieved" for cell in result["cells"]):
        raise ValueError("measured denominator inconsistent")
    expected_state = "completed" if result["measured"] == result["selected"] else "partial"
    if result["state"] != expected_state:
        raise ValueError("inconsistent completeness")
    expected = {"case", "provider", "outcome", "evidence", "measurements"}
    for cell in result["cells"]:
        if set(cell) != expected:
            raise ValueError("unexpected cell fields")
        if cell["provider"] != "exa" or cell["outcome"] not in ("retrieved", "refused", "unavailable"):
            raise ValueError("unsupported cell outcome")
        metrics = {"response_bytes", "result_limit_requested", "result_limit_effective", "results_returned",
                   "extracted_text", "admitted_context", "model_input", "model_output", "judge_tokens",
                   "charge", "supplier_latency"}
        if set(cell["measurements"]) != metrics or set(cell["evidence"]) != {"summary", "sha256"}:
            raise ValueError("invalid cell projection")
        for item in cell["measurements"].values():
            if set(item) != {"value", "unit", "basis", "missing_reason"}:
                raise ValueError("invalid measurement shape")
            if item["basis"] not in ("observed", "quoted", "estimated", "unavailable"):
                raise ValueError("unknown measurement basis")
            if item["value"] is None and not item["missing_reason"]:
                raise ValueError("unknown measurement needs a reason")
            if item["value"] is not None and type(item["value"]) not in (int, float):
                raise ValueError("non-numerical measurement")


def run(queue, scope, execution, binary, crash=None):
    token = queue.claim(scope, execution)
    row = queue.get(scope, execution)
    config = json.loads(row["definition"])
    if digest(canonical(config)) != row["version"]:
        raise ValueError("definition integrity failure")
    output = queue.root / execution
    output.mkdir(mode=0o700)
    home = output / "home"
    home.mkdir(mode=0o700)
    replay = output / "fixture"
    fixture(replay)
    if crash == "before-send":
        os._exit(71)
    # This intent boundary precedes process launch: dispatched does not prove HTTP send.
    queue.transition(scope, execution, token, "reserved", "dispatched")
    cells = []
    for case in config["cases"]:
        # Recheck local authority before each child, without claiming remote fencing.
        current = queue.get(scope, execution)
        if current["token"] != token or current["state"] != "dispatched":
            raise ValueError("stale worker; no subsequent child dispatch")
        job = output / f"{len(cells)}.suite.json"
        atomic_json(job, suite(config, case))
        destination = output / f"{len(cells)}.run"
        subprocess.run([str(Path(binary).resolve()), "run", str(job), "--replay", str(replay),
                        "--output", str(destination)], cwd=home,
                       env={"HOME": str(home), "COMMONMEASURE_HOME": str(home),
                            "PI_CODING_AGENT_DIR": str(home / "pi"),
                            "CLAUDE_CONFIG_DIR": str(home / "claude"), "CODEX_HOME": str(home / "codex")},
                       check=True, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, timeout=60)
        if crash == "after-child":
            # The real replay CLI has published evidence; the queue has not indexed it.
            os._exit(72)
        summary_bytes = (destination / "summary.json").read_bytes()
        summary = json.loads(summary_bytes)
        if "sha256:" + digest((destination / "evidence.ndjson").read_bytes()) != summary["evidence_log"]["sha256"]:
            raise ValueError("source evidence integrity failure")
        if not summary["run"]["evidence_complete"]:
            raise ValueError("source evidence incomplete; stop later dispatch")
        plan = next(p for p in summary["plans"] if p["provider"] == "exa")
        acquisition = plan["acquisition"]
        acquired = acquisition is not None and acquisition.get("response_hash") is not None
        if acquired:
            response = json.loads((destination / acquisition["response_ref"]).read_bytes())
            if "sha256:" + digest(canonical(response)) != acquisition["response_hash"]:
                raise ValueError("response integrity failure")
        admitted = any(s["admitted"] for s in plan["sources"])
        outcome = "retrieved" if acquired and admitted else plan["status"]
        cells.append({"case": case["id"], "provider": "exa", "outcome": outcome,
                      "evidence": {"summary": f"{len(cells)}.run/summary.json", "sha256": digest(summary_bytes)},
                      "measurements": {
                          "response_bytes": measurement(acquisition["response_bytes"] if acquired else None,
                                                        "bytes", "observed" if acquired else "unavailable", "no response"),
                          "result_limit_requested": measurement(config["result_limit"], "results", "observed"),
                          "result_limit_effective": measurement(None, "results", "unavailable",
                                                                "batch record does not assert supplier enforcement"),
                          "results_returned": measurement(acquisition["result_count"] if acquired else None,
                                                         "results", "observed" if acquired else "unavailable", "no response"),
                          "extracted_text": measurement(None, "whitespace words", "unavailable",
                                                        "no separate pre-transformation count in this projection"),
                          "admitted_context": measurement(plan["context_tokens_admitted"] if acquired else None,
                                                          "whitespace words", "estimated" if acquired else "unavailable",
                                                          "no acquisition"),
                          "model_input": measurement(None, "provider tokens", "unavailable", "inference disabled"),
                          "model_output": measurement(None, "provider tokens", "unavailable", "inference disabled"),
                          "judge_tokens": measurement(None, "provider tokens", "unavailable", "no judge"),
                          "charge": measurement(None, "USD", "unavailable", "synthetic fixture reports no charge"),
                          "supplier_latency": measurement(None, "ms", "unavailable", "loopback is not supplier timing")}})
    measured = sum(cell["outcome"] == "retrieved" for cell in cells)
    state = "completed" if measured == len(cells) else "partial"
    result = {"schema_version": "contextops-local-sweep/v1", "execution": execution,
              "version": row["version"],
              "producer": {"binary_sha256": digest(Path(binary).read_bytes()),
                           "wrapper_sha256": digest(Path(__file__).read_bytes())},
              "state": state, "selected": len(config["cases"]),
              "measured": measured, "cells": cells}
    validate_export(result)
    atomic_json(output / "export.json", result)
    queue.transition(scope, execution, token, "dispatched", state, digest(canonical(result)))
    if crash == "after-evidence":
        os._exit(73)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--use-case", choices=["agency", "supplier"], default="agency")
    parser.add_argument("--crash", choices=["before-send", "after-child", "after-evidence"])
    args = parser.parse_args()
    queue = Queue(args.root)
    scope = ("synthetic-org", args.use_case)
    try:
        queue.project(scope, 2)
    except sqlite3.IntegrityError:
        pass
    execution = queue.submit(scope, "manual-1", definition(args.use_case))
    row = queue.get(scope, execution)
    if row["state"] == "queued":
        run(queue, scope, execution, args.binary, args.crash)
    elif row["state"] in ("reserved", "dispatched"):
        queue.recover(scope, execution)
    row = queue.get(scope, execution)
    print(json.dumps({key: row[key] for key in ("id", "version", "state", "held", "export_hash")},
                     sort_keys=True))
    queue.close()


if __name__ == "__main__":
    main()
