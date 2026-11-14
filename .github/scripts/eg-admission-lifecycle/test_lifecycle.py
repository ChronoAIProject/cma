"""Pure clock/receipt and mocked Kubernetes tests; no process or network fixtures."""
import hashlib
import json
from pathlib import Path
import tempfile
import types
import unittest
from unittest.mock import patch
import subprocess
import uuid

from binding import project_pods
from capture import capture
from lifecycle import BINDINGS, COUNTS, PHASES, EvidenceError, Lifecycle, validate_plan


H = "a" * 64
INSTANCE = "b" * 64


def plan():
    return {"schema": "eg_lifecycle_plan_v1", "run_id": str(uuid.uuid4()), "cohort_id": str(uuid.uuid4()),
            "replicas": 2, "candidate_commit": "c" * 40, "candidate_tree": "d" * 40,
            "bindings": dict.fromkeys(BINDINGS, H), "budgets_ms": dict.fromkeys(PHASES, 100),
            "return_bounds_ms": dict.fromkeys(PHASES, 110),
            "expected_timeout_phase": None, "departing_instances": [INSTANCE]}


def accounting():
    result = dict.fromkeys(COUNTS, 0)
    result.update(http_attempts=2, submitted_events=1, custody_established=1,
                  direct_terminal=1, expected_effects=1, confirmed_effects=1, receipt_sha256=H)
    return result


def custody():
    return {"receipt_sha256": H, "registered_departures": [INSTANCE],
            "confirmed_process_exits": [INSTANCE], "collectors_closed": True, "fixture_closed": True,
            "no_late_effect_barrier": True, "resources_disposed": False}


class LedgerTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="eg-lifecycle-", dir=Path(__file__).parent)
        self.addCleanup(self.tmp.cleanup)
        self.now = 0.0

    def ledger(self, value=None):
        ledger = Lifecycle(Path(self.tmp.name) / str(uuid.uuid4()), value or plan(), lambda: self.now,
                           lambda: "2026-11-14T00:00:00+00:00")
        self.addCleanup(ledger.close)
        return ledger

    def finish(self, ledger, failed=None, timeout_seconds=0.101):
        for phase in PHASES:
            if phase == "local_destroy":
                ledger.confirm_custody(custody())
            ledger.start(phase)
            self.now += timeout_seconds if phase == failed else 0.001
            ledger.end(phase, "timed_out" if phase == failed else "ok", H)

    def test_complete_receipts_do_not_automatically_accept_application(self):
        ledger = self.ledger()
        self.finish(ledger)
        proof = custody(); proof["resources_disposed"] = True
        result = ledger.finalize(accounting(), proof)
        self.assertEqual(result["evidence_status"], "COMPLETE")
        self.assertEqual(result["application_acceptance"], "REQUIRES_INDEPENDENT_REVIEW")
        rows = [json.loads(line) for line in (ledger.directory / "events.jsonl").read_text().splitlines()]
        self.assertEqual([row["sequence"] for row in rows], list(range(len(rows))))
        confirmed = next(i for i, row in enumerate(rows) if row["boundary"] == "custody")
        destroy = next(i for i, row in enumerate(rows) if row["phase"] == "local_destroy")
        self.assertLess(confirmed, destroy)
        self.assertEqual(hashlib.sha256((ledger.directory / "events.jsonl").read_bytes()).hexdigest(),
                         result["events_sha256"])
        with self.assertRaises(EvidenceError): ledger.start("prepare")

    def test_interruption_retains_start_without_inventing_end(self):
        ledger = self.ledger(); ledger.start("prepare"); ledger.close()
        rows = (ledger.directory / "events.jsonl").read_text().splitlines()
        self.assertEqual(len(rows), 1)
        self.assertEqual(json.loads(rows[0])["boundary"], "start")
        self.assertFalse((ledger.directory / "result.json").exists())

    def test_stall_requires_registered_phase_and_elapsed_budget(self):
        value = plan(); value["expected_timeout_phase"] = "child_wait"
        ledger = self.ledger(value); self.finish(ledger, "child_wait")
        proof = custody(); proof["resources_disposed"] = True
        result = ledger.finalize(accounting(), proof)
        self.assertTrue(result["registered_stall_observed"])
        self.assertFalse(result["normal_boundaries_observed"])
        self.assertEqual(result["evidence_status"], "COMPLETE")
        timeout = next(row for row in result["phases"] if row["phase"] == "child_wait")
        self.assertEqual(timeout["observed_status"], "timed_out")
        self.assertTrue(timeout["returned_within_bound"])
        early = self.ledger(); early.start("prepare")
        with self.assertRaisesRegex(EvidenceError, "premature_timeout"): early.end("prepare", "timed_out", H)

    def test_missing_expected_stall_is_incomplete(self):
        value = plan(); value["expected_timeout_phase"] = "child_wait"
        ledger = self.ledger(value); self.finish(ledger)
        proof = custody(); proof["resources_disposed"] = True
        self.assertEqual(ledger.finalize(accounting(), proof)["evidence_status"], "INCOMPLETE")

    def test_late_success_cannot_hide_exceeded_deadline(self):
        ledger = self.ledger(); ledger.start("prepare"); self.now += 0.2
        ledger.end("prepare", "ok", H)
        self.assertEqual(ledger.completed[0]["status"], "late_return")

    def test_vastly_late_timeout_is_observed_but_cannot_complete_evidence(self):
        value = plan(); value["expected_timeout_phase"] = "child_wait"
        ledger = self.ledger(value); self.finish(ledger, "child_wait", timeout_seconds=3600)
        proof = custody(); proof["resources_disposed"] = True
        result = ledger.finalize(accounting(), proof)
        timeout = next(row for row in result["phases"] if row["phase"] == "child_wait")
        self.assertEqual(timeout["observed_status"], "timed_out")
        self.assertEqual(timeout["status"], "late_return")
        self.assertFalse(timeout["returned_within_bound"])
        self.assertFalse(result["registered_stall_observed"])
        self.assertTrue(result["custody_confirmed"])
        self.assertEqual(result["evidence_status"], "INCOMPLETE")
        events = [json.loads(line) for line in (ledger.directory / "events.jsonl").read_text().splitlines()]
        end = next(row for row in events if row["phase"] == "child_wait" and row["boundary"] == "end")
        self.assertEqual((end["observed_status"], end["status"]), ("timed_out", "late_return"))

    def test_return_bound_is_explicit_and_does_not_extend_success_budget(self):
        for status, seconds, expected in (("timed_out", .125, "timed_out"),
                                          ("timed_out", .25, "timed_out"),
                                          ("timed_out", .251, "late_return"),
                                          ("ok", .2, "late_return"), ("failed", .251, "late_return")):
            with self.subTest(status=status, seconds=seconds):
                self.now = 0.0
                value = plan(); value["budgets_ms"]["prepare"] = 125
                value["return_bounds_ms"]["prepare"] = 250
                ledger = self.ledger(value); ledger.start("prepare"); self.now = seconds
                ledger.end("prepare", status, H)
                self.assertEqual(ledger.completed[0]["status"], expected)
        for invalid in (None, {}, dict.fromkeys(PHASES, 99), dict.fromkeys(PHASES, 120001),
                        dict.fromkeys(PHASES, True)):
            value = plan(); value["return_bounds_ms"] = invalid
            with self.subTest(invalid=invalid), self.assertRaisesRegex(EvidenceError, "return_bound"):
                validate_plan(value)
        value = plan(); del value["return_bounds_ms"]
        with self.assertRaisesRegex(EvidenceError, "plan_fields"): validate_plan(value)

    def test_disposal_requires_independent_registered_custody(self):
        ledger = self.ledger()
        for phase in PHASES[:7]:
            ledger.start(phase); ledger.end(phase, "ok", H)
        proof = custody(); proof["confirmed_process_exits"] = []
        self.assertFalse(ledger.confirm_custody(proof))
        with self.assertRaisesRegex(EvidenceError, "custody_unconfirmed"): ledger.start("local_destroy")
        proof["resources_disposed"] = True
        result = ledger.finalize(accounting(), proof)
        self.assertTrue(result["disposal_without_confirmation"])
        self.assertEqual(result["evidence_status"], "INCOMPLETE")

    def test_cannot_remove_registered_departure_or_add_raw_fields(self):
        ledger = self.ledger()
        proof = custody(); proof["registered_departures"] = []
        with self.assertRaisesRegex(EvidenceError, "registered_departures_changed"): ledger.confirm_custody(proof)
        value = plan(); value["credentials"] = "private"
        with self.assertRaises(EvidenceError): validate_plan(value)
        with self.assertRaises(EvidenceError): ledger.end("prepare", "ok", H)
        with self.assertRaises(EvidenceError): ledger.start("workload")

    def test_attempts_events_and_terminal_buckets_are_distinct(self):
        ledger = self.ledger(); self.finish(ledger)
        bad = accounting(); bad["submitted_events"] = 2
        with self.assertRaisesRegex(EvidenceError, "admission_partition"): ledger.finalize(bad, custody())
        bad = accounting(); bad["reconciled_terminal"] = 1
        with self.assertRaisesRegex(EvidenceError, "custody_partition"): ledger.finalize(bad, custody())
        pending = accounting(); pending.update(direct_terminal=0, unresolved=1, confirmed_effects=0)
        self.assertFalse(ledger.finalize(pending, custody())["accounting_closed"])

    def test_ambiguous_admission_is_not_dropped_from_denominator(self):
        ledger = self.ledger(); self.finish(ledger)
        pending = accounting(); pending.update(custody_established=0, direct_terminal=0, admission_uncertain=1)
        self.assertFalse(ledger.finalize(pending, custody())["accounting_closed"])

    def test_clock_regression_and_repeat_directory_refused(self):
        ledger = self.ledger(); ledger.start("prepare"); self.now -= 1
        with self.assertRaisesRegex(EvidenceError, "clock_regressed"): ledger.end("prepare", "ok", H)
        with self.assertRaises(FileExistsError): Lifecycle(ledger.directory, plan())

    def test_failed_request_still_requires_terminal_and_shutdown_phases(self):
        ledger = self.ledger()
        for phase in PHASES:
            if phase == "local_destroy": ledger.confirm_custody(custody())
            ledger.start(phase); ledger.end(phase, "failed" if phase == "workload" else "ok", H)
        result = ledger.finalize(accounting(), custody())
        self.assertTrue(result["complete"])
        self.assertEqual(result["evidence_status"], "INCOMPLETE")
        self.assertIn("terminal_wait", [row["phase"] for row in result["phases"]])


def pod(index):
    return {"metadata": {"uid": "pod-" + str(index), "namespace": "cma-scale"},
            "spec": {"nodeName": "node-" + str(index)},
            "status": {"phase": "Running", "conditions": [{"type": "Ready", "status": "True"}],
                       "containerStatuses": [{"name": "gateway", "ready": True, "imageID": "runtime://sha256:" + H,
                       "containerID": "container-" + str(index), "restartCount": 0,
                       "state": {"running": {"startedAt": "2026-11-14T00:00:00Z"}}}]}}


class BindingTests(unittest.TestCase):
    def project(self, pods):
        return project_pods(pods, replicas=2, image_digest=H, namespace_uid="ns", container_name="gateway")

    def test_restart_changes_instance_and_raw_names_not_retained(self):
        original = self.project([pod(1), pod(2)])
        changed = pod(1); changed["status"]["containerStatuses"][0]["restartCount"] = 1
        after = self.project([changed, pod(2)])
        self.assertNotEqual(original["pods"], after["pods"])
        self.assertNotIn('"pod-1"', json.dumps(original))
        self.assertEqual(original["distinct_nodes"], 2)
        self.assertFalse(original["process_exit_observed"])

    def test_absent_wrong_image_duplicate_and_terminating_pods_rejected(self):
        for pods in [[], [pod(1), pod(1)], [pod(1)]]:
            with self.subTest(pods=len(pods)), self.assertRaises(EvidenceError): self.project(pods)
        for field, value in [("imageID", "sha256:" + "f" * 64), ("ready", False)]:
            changed = pod(1); changed["status"]["containerStatuses"][0][field] = value
            with self.subTest(field=field), self.assertRaises(EvidenceError): self.project([changed, pod(2)])
        changed = pod(1); changed["metadata"]["deletionTimestamp"] = "2026-11-14T00:01:00Z"
        with self.assertRaises(EvidenceError): self.project([changed, pod(2)])

    def test_colocation_is_reported_without_inventing_placement_pass(self):
        changed = pod(2); changed["spec"]["nodeName"] = "node-1"
        self.assertEqual(self.project([pod(1), changed])["distinct_nodes"], 1)

    def test_capture_pins_scope_uses_actual_selector_and_never_reads_secrets(self):
        value = plan(); value["bindings"]["namespace"] = hashlib.sha256(b"ns").hexdigest()
        calls = []
        def execute(args, **options):
            calls.append(args)
            self.assertEqual(options["timeout"], 15)
            if "view" in args: text = "https://127.0.0.1:16443"
            elif "namespace" in args[8:]: text = "ns"
            elif "deployment" in args: text = '{"actual":"selector"}'
            else:
                self.assertIn("actual=selector", args)
                records = []
                for item in [pod(1), pod(2)]:
                    records.append("\t".join([item["metadata"]["uid"], "cma-scale", "", item["spec"]["nodeName"],
                                             "Running", json.dumps(item["status"]["conditions"]),
                                             json.dumps(item["status"]["containerStatuses"])]))
                text = "\n".join(records)
            return types.SimpleNamespace(returncode=0, stdout=text)
        result = capture(value, "/private/kubeconfig", "gateway", "/bin/kubectl", execute)
        self.assertEqual(len(calls), 5)
        self.assertTrue(result["read_only"])
        self.assertNotIn("secrets", json.dumps(calls))
        self.assertTrue(all("kind-cma-scale" in args and "cma-scale" in args for args in calls))

    def test_remote_cluster_refused_before_resource_read(self):
        calls = []
        def execute(args, **_):
            calls.append(args)
            return types.SimpleNamespace(returncode=0, stdout="https://remote.invalid")
        with self.assertRaisesRegex(EvidenceError, "nonlocal_cluster"):
            capture(plan(), "/private/config", "gateway", "/bin/kubectl", execute)
        self.assertEqual(len(calls), 1)

    def test_capture_timeout_retains_partial_read_observation_without_error_text(self):
        events = []
        def execute(args, **_):
            raise subprocess.TimeoutExpired(args, 15, output="private-body", stderr="private-credential")
        with self.assertRaises(subprocess.TimeoutExpired):
            capture(plan(), "/private/config", "gateway", "/bin/kubectl", execute, events.append)
        self.assertEqual([row["boundary"] for row in events], ["start", "end"])
        self.assertEqual(events[-1]["status"], "read_incomplete")
        self.assertNotIn("private", json.dumps(events))

    def test_capture_cli_preserves_safe_failure_receipt_and_journal(self):
        from capture import main
        with tempfile.TemporaryDirectory(prefix="eg-capture-", dir=Path(__file__).parent) as tmp:
            source = Path(tmp) / "plan.json"; source.write_text(json.dumps(plan()))
            output = Path(tmp) / "result.json"
            def failed(*_args, record, **_kwargs):
                record({"ordinal": 0, "boundary": "start"})
                raise ValueError("private-credential")
            args = ["capture.py", "--plan", str(source), "--out", str(output), "--kubeconfig",
                    "/private/config", "--kubectl", "/bin/kubectl", "--container", "gateway"]
            with patch("sys.argv", args), patch("capture.capture", failed), patch("builtins.print"):
                self.assertEqual(main(), 2)
            result = json.loads(output.read_text())
            self.assertEqual(result["code"], "capture_failed")
            self.assertNotIn("private", output.read_text())
            journal = output.with_suffix(".json.observations.jsonl")
            self.assertEqual(hashlib.sha256(journal.read_bytes()).hexdigest(), result["observation_journal_sha256"])


if __name__ == "__main__":
    unittest.main()
