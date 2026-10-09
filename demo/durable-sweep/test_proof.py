"""Synthetic replay and queue-boundary checks; no live integration claims."""
import concurrent.futures
import copy
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import proof

BINARY = Path(os.environ.get("SWEEP_BINARY", "target/debug/commonmeasure")).resolve()


class QueueTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.queue = proof.Queue(self.root)
        self.scope = ("org-a", "project-a")
        self.queue.project(self.scope, 2)

    def tearDown(self):
        self.queue.close()
        self.temp.cleanup()

    def submit(self, slot="slot-1", config=None):
        return self.queue.submit(self.scope, slot, config or proof.definition("agency"))

    def test_duplicate_slot_and_definition_version(self):
        execution = self.submit()
        self.assertEqual(execution, self.submit())
        for change in ("question", "context_words", "constraints"):
            config = proof.definition("agency")
            if change == "question":
                config["cases"][0][change] += " Changed"
            elif change == "context_words":
                config[change] = 32
            else:
                config[change] = [{"kind": "denied_provider", "provider": "exa"}]
            with self.assertRaisesRegex(ValueError, "another definition"):
                self.submit(config=config)
            second = self.submit(slot=change, config=config)
            self.assertNotEqual(self.queue.get(self.scope, execution)["version"],
                                self.queue.get(self.scope, second)["version"])

    def test_scope_guards_and_revocation(self):
        execution = self.submit()
        for scope in (("org-b", "project-a"), ("org-a", "project-b")):
            self.queue.project(scope, 2)
            with self.assertRaises(PermissionError):
                self.queue.get(scope, execution)
            with self.assertRaises(PermissionError):
                self.queue.export(scope, execution)
        token = self.queue.claim(self.scope, execution)
        self.queue.revoke(self.scope)
        with self.assertRaises(PermissionError):
            self.queue.transition(self.scope, execution, token, "reserved", "dispatched")

    def test_concurrent_submission_and_reservations(self):
        def submit_same(_):
            queue = proof.Queue(self.root)
            try:
                return queue.submit(self.scope, "duplicate", proof.definition("agency"))
            finally:
                queue.close()
        with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
            ids = list(pool.map(submit_same, range(2)))
        self.assertEqual(ids[0], ids[1])
        other = self.submit("other")

        def claim(execution):
            queue = proof.Queue(self.root)
            try:
                queue.claim(self.scope, execution)
                return "reserved"
            except ValueError as error:
                self.assertIn("allowance exhausted", str(error))
                return "refused"
            finally:
                queue.close()
        with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
            outcomes = list(pool.map(claim, [ids[0], other]))
        self.assertCountEqual(outcomes, ["reserved", "refused"])
        held = self.queue.db.execute("SELECT SUM(held) FROM executions").fetchone()[0]
        self.assertEqual(held, 2)

    def test_unknown_price_never_establishes_money_ceiling(self):
        config = proof.definition("agency")
        config["money_ceiling"] = {"currency": "USD", "micros": 1}
        with self.assertRaisesRegex(ValueError, "no enforceable monetary bound"):
            self.submit(config=config)
        self.assertEqual(self.queue.db.execute("SELECT COUNT(*) FROM executions").fetchone()[0], 0)

    def test_recovery_preserves_unknown_exposure_and_fences_old_writer(self):
        execution = self.submit()
        token = self.queue.claim(self.scope, execution)
        self.queue.transition(self.scope, execution, token, "reserved", "dispatched")
        self.assertEqual(self.queue.recover(self.scope, execution), "outcome_unknown")
        self.assertEqual(self.queue.get(self.scope, execution)["held"], 2)
        with self.assertRaisesRegex(ValueError, "stale worker"):
            self.queue.transition(self.scope, execution, token, "dispatched", "completed")
        with self.assertRaisesRegex(ValueError, "never automatically retried"):
            self.queue.claim(self.scope, execution)
        other = self.submit("later")
        with self.assertRaisesRegex(ValueError, "allowance exhausted"):
            self.queue.claim(self.scope, other)

    def test_before_send_releases_only_known_unsent_work(self):
        execution = self.submit()
        token = self.queue.claim(self.scope, execution)
        self.assertEqual(self.queue.recover(self.scope, execution), "not_sent")
        self.assertEqual(self.queue.get(self.scope, execution)["held"], 0)
        with self.assertRaisesRegex(ValueError, "stale worker"):
            self.queue.transition(self.scope, execution, token, "reserved", "dispatched")


class ReplayTests(unittest.TestCase):
    tearDown = QueueTests.tearDown
    submit = QueueTests.submit

    def setUp(self):
        QueueTests.setUp(self)
        if not BINARY.is_file():
            self.fail("build commonmeasure-cli or set SWEEP_BINARY; replay tests are not optional")

    def test_both_configurations_same_path_and_metrics_only_exports(self):
        original = copy.deepcopy(dict(os.environ))
        with patch.dict(os.environ, {"COMMONMEASURE_INFERENCE_ENDPOINT": "https://must-not-be-used.invalid",
                                     "EXA_API_KEY": "synthetic-forbidden-parent-value"}):
            for use_case in ("agency", "supplier"):
                scope = ("org-a", use_case)
                self.queue.project(scope, 2)
                execution = self.queue.submit(scope, "manual", proof.definition(use_case))
                result = proof.run(self.queue, scope, execution, BINARY)
                self.assertEqual(result["selected"], 2)
                self.assertEqual(result["measured"], 2)
                self.assertEqual(result["state"], "completed")
                self.assertEqual(self.queue.export(scope, execution), result)
                for index, cell in enumerate(result["cells"]):
                    source = self.root / execution / cell["evidence"]["summary"]
                    self.assertEqual(proof.digest(source.read_bytes()), cell["evidence"]["sha256"])
                    summary = json.loads(source.read_bytes())
                    self.assertFalse(summary["model_plan"]["configured"])
                    plan = next(p for p in summary["plans"] if p["provider"] == "exa")
                    self.assertEqual(plan["verification_state"], "replay-tested")
                    self.assertIsNone(plan["inference"])
                    self.assertIsNone(plan["answer"])
                    self.assertTrue(plan["acquisition"]["replay"]["matches_recorded_input"])
                    self.assertIsNone(plan["acquisition"]["charge"].get("money"))
                    self.assertIsNone(plan["acquisition"]["charge"].get("native"))
                    self.assertIsNone(cell["measurements"]["charge"]["value"])
                    self.assertIsNone(cell["measurements"]["model_input"]["value"])
                    self.assertEqual(cell["measurements"]["admitted_context"]["unit"], "whitespace words")
                    response = self.root / execution / f"{index}.run" / plan["acquisition"]["response_ref"]
                    self.assertEqual("sha256:" + proof.digest(proof.canonical(json.loads(response.read_bytes()))),
                                     plan["acquisition"]["response_hash"])
                body = proof.canonical(result).decode()
                for forbidden in ("Who makes", "Cobalt", "example.invalid", "synthetic-forbidden-parent-value"):
                    self.assertNotIn(forbidden, body)
                for path in (self.root / execution).rglob("*"):
                    if path.is_file():
                        self.assertNotIn(b"synthetic-forbidden-parent-value", path.read_bytes())
                with self.assertRaisesRegex(ValueError, "never automatically retried"):
                    proof.run(self.queue, scope, execution, BINARY)
        self.assertEqual(dict(os.environ), original)

    def test_policy_refusal_keeps_selected_denominator(self):
        config = proof.definition("agency")
        config["constraints"] = [{"kind": "denied_provider", "provider": "exa"}]
        execution = self.submit(config=config)
        result = proof.run(self.queue, self.scope, execution, BINARY)
        self.assertEqual(result["state"], "partial")
        self.assertEqual(result["selected"], 2)
        self.assertEqual(result["measured"], 0)
        self.assertTrue(all(c["outcome"] == "refused" for c in result["cells"]))
        self.assertFalse(list((self.root / execution).glob("*.run/responses/*.json")))
        self.assertTrue(all(c["measurements"]["response_bytes"]["value"] is None for c in result["cells"]))

    def test_actual_process_exit_at_each_persisted_boundary(self):
        for boundary, code, state, held in (("before-send", 71, "not_sent", 0),
                                            ("after-child", 72, "outcome_unknown", 2),
                                            ("after-evidence", 73, "completed", 2)):
            root = self.root / boundary
            child = subprocess.run([sys.executable, str(proof.HERE / "proof.py"), "--root", str(root),
                                    "--binary", str(BINARY), "--crash", boundary],
                                   env={"HOME": str(self.root)}, capture_output=True, timeout=60)
            self.assertEqual(child.returncode, code, child.stderr.decode())
            queue = proof.Queue(root)
            scope = ("synthetic-org", "agency")
            execution = queue.submit(scope, "manual-1", proof.definition("agency"))
            self.assertEqual(queue.recover(scope, execution), state)
            self.assertEqual(queue.get(scope, execution)["held"], held)
            if boundary == "before-send":
                self.assertFalse(list(root.glob("*/*.run")))
            else:
                self.assertTrue(list(root.glob("*/*.run/summary.json")))
            if boundary == "after-child":
                self.assertFalse((root / execution / "export.json").exists())
            if state == "completed":
                self.assertEqual(queue.export(scope, execution)["measured"], 2)
            with self.assertRaisesRegex(ValueError, "never automatically retried"):
                queue.claim(scope, execution)
            queue.close()

    def test_failed_export_write_holds_exposure_and_no_retry(self):
        execution = self.submit()
        write = proof.atomic_json
        def fail_export(path, value):
            if path.name == "export.json":
                raise OSError("injected evidence write failure")
            write(path, value)
        with patch.object(proof, "atomic_json", fail_export):
            with self.assertRaisesRegex(OSError, "evidence write failure"):
                proof.run(self.queue, self.scope, execution, BINARY)
        self.assertEqual(self.queue.recover(self.scope, execution), "outcome_unknown")
        self.assertEqual(self.queue.get(self.scope, execution)["held"], 2)
        with self.assertRaisesRegex(ValueError, "no indexed export"):
            self.queue.export(self.scope, execution)

    def test_replaced_worker_stops_before_subsequent_replay_child(self):
        execution = self.submit()
        launch = proof.subprocess.run
        def replace_after_child(*args, **kwargs):
            result = launch(*args, **kwargs)
            self.queue.recover(self.scope, execution)
            return result
        with patch.object(proof.subprocess, "run", side_effect=replace_after_child) as child:
            with self.assertRaisesRegex(ValueError, "stale worker"):
                proof.run(self.queue, self.scope, execution, BINARY)
            self.assertEqual(child.call_count, 1)
        self.assertEqual(self.queue.get(self.scope, execution)["state"], "outcome_unknown")
        self.assertFalse((self.root / execution / "1.run").exists())

    def test_timeout_is_uncertain_without_automatic_retry(self):
        execution = self.submit()
        with patch.object(proof.subprocess, "run", side_effect=subprocess.TimeoutExpired("cli", 60)) as child:
            with self.assertRaises(subprocess.TimeoutExpired):
                proof.run(self.queue, self.scope, execution, BINARY)
            self.assertEqual(child.call_count, 1)
        self.assertEqual(self.queue.recover(self.scope, execution), "outcome_unknown")
        self.assertEqual(self.queue.get(self.scope, execution)["held"], 2)

    def test_corrupt_replay_refused_by_production_without_fallback(self):
        execution = self.submit()
        fixture = proof.fixture
        def corrupt(directory):
            fixture(directory)
            path = directory / "capture.json"
            capture = json.loads(path.read_bytes())
            capture["response"]["body"]["results"][0]["text"] = "Altered synthetic bytes"
            path.write_bytes(proof.canonical(capture))
        with patch.object(proof, "fixture", corrupt):
            with self.assertRaises(subprocess.CalledProcessError) as error:
                proof.run(self.queue, self.scope, execution, BINARY)
        self.assertIn(b"does not match the manifest", error.exception.stderr)
        self.assertFalse(list((self.root / execution).glob("*.run/summary.json")))
        self.assertEqual(self.queue.recover(self.scope, execution), "outcome_unknown")

    def test_tampered_export_refused_without_reexecution(self):
        execution = self.submit()
        proof.run(self.queue, self.scope, execution, BINARY)
        (self.root / execution / "export.json").write_text("{}")
        with self.assertRaisesRegex(ValueError, "integrity failure"):
            self.queue.export(self.scope, execution)


if __name__ == "__main__":
    unittest.main()
