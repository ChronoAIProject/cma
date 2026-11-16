import contextlib
import copy
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from hook_events import Monitor, classify, target
from watch_hook_events import parser, read_events, run

NOW = 1794828600
SCOPE = target("am:local-rehearsal:cma-agent-manager-")


def event(reason="FailedPreStopHook", count=1):
    return {"metadata": {"uid": "event-one", "namespace": "local-rehearsal"},
            "involvedObject": {"kind": "Pod", "name": "cma-agent-manager-old-abcde",
                               "namespace": "local-rehearsal", "uid": "pod-original"},
            "type": "Warning", "reason": reason, "count": count,
            "lastTimestamp": "2026-11-16T11:29:58Z",
            "message": "Exec lifecycle hook failed; command/private data must not escape"}


def listing(*events):
    return {"kind": "EventList", "items": list(events), "metadata": {"resourceVersion": "123"}}


class ClassifierTests(unittest.TestCase):
    def setUp(self):
        self.monitor = Monitor([SCOPE], NOW - 3600, "fixture")

    def test_deleted_pod_and_main_exit_zero_do_not_hide_hook_failure(self):
        rows = self.monitor.consume(SCOPE["namespace"], listing(event()), NOW)
        self.assertEqual(len(rows), 1)
        self.assertEqual(rows[0]["pod_uid"], "pod-original")
        self.assertEqual(rows[0]["category"], "pre_stop")
        self.assertEqual(rows[0]["event_observation_to_detection_seconds"], 2)
        self.assertIsNone(rows[0]["fault_to_detection_seconds"])
        self.assertIsNone(rows[0]["mtta_seconds"])
        self.assertEqual(rows[0]["delivery"], "NOT_SENT")
        self.assertNotIn("command/private", json.dumps(rows))

    def test_events_v1_series_updates_and_restart_deduplicate(self):
        row = event()
        row["regarding"] = row.pop("involvedObject")
        row["note"] = row.pop("message")
        row["series"] = {"count": 2, "lastObservedTime": row.pop("lastTimestamp")}
        self.assertEqual(len(self.monitor.consume(SCOPE["namespace"], listing(row), NOW)), 1)
        monitor = Monitor([SCOPE], NOW, "fixture", copy.deepcopy(self.monitor.state))
        self.assertEqual(monitor.consume(SCOPE["namespace"], listing(row), NOW), [])
        row["series"]["count"] = 3
        self.assertEqual(len(monitor.consume(SCOPE["namespace"], listing(row), NOW)), 1)
        self.assertIn('service="am",reason="pre_stop"} 2', monitor.metrics(NOW))

    def test_same_pod_name_with_new_uid_is_a_distinct_failure(self):
        first, second = event(), event()
        second["metadata"]["uid"] = "event-two"
        second["involvedObject"]["uid"] = "pod-recreated"
        self.assertEqual(len(self.monitor.consume(SCOPE["namespace"], listing(first, second), NOW)), 2)

    def test_explicit_drain_failures_and_warning_notes(self):
        for reason in ["DrainHookFailed", "FailedDrainHook", "DrainFailed", "DrainTimeout"]:
            self.assertEqual(classify(event(reason)), "drain")
        row = event("LifecycleHook")
        row["message"] = "draining failed: deadline exceeded"
        self.assertEqual(classify(row), "drain")
        row["message"] = "drain completed successfully"
        self.assertIsNone(classify(row))
        row["message"] = "failed to pull an image"
        self.assertIsNone(classify(row))

    def test_scope_old_events_and_future_clock(self):
        row = event()
        row["involvedObject"]["namespace"] = "other"
        self.assertEqual(self.monitor.consume(SCOPE["namespace"], listing(row), NOW), [])
        row = event()
        row["lastTimestamp"] = "2026-11-15T11:29:58Z"
        self.assertEqual(self.monitor.consume(SCOPE["namespace"], listing(row), NOW), [])
        row["lastTimestamp"] = "2027-11-15T11:29:58Z"
        with self.assertRaisesRegex(ValueError, "event_clock_ahead"):
            self.monitor.consume(SCOPE["namespace"], listing(row), NOW)

    def test_missing_time_alerts_without_fabricated_latency(self):
        row = event()
        del row["lastTimestamp"]
        result = self.monitor.consume(SCOPE["namespace"], listing(row), NOW)
        self.assertIsNone(result[0]["event_observation_to_detection_seconds"])

    def test_incomplete_or_malformed_list_never_records_success(self):
        row = event()
        del row["metadata"]["uid"]
        with self.assertRaises(ValueError):
            self.monitor.consume(SCOPE["namespace"], listing(event(), row), NOW)
        self.assertEqual(self.monitor.state["seen"], {})
        self.assertEqual(self.monitor.up[SCOPE["namespace"]], 0)
        bad = listing()
        bad["metadata"]["continue"] = "not-complete"
        with self.assertRaisesRegex(ValueError, "events_list_incomplete"):
            self.monitor.consume(SCOPE["namespace"], bad, NOW)

    def test_gap_metric_does_not_overwrite_last_success(self):
        self.monitor.consume(SCOPE["namespace"], listing(), NOW)
        gap = self.monitor.failure(SCOPE["namespace"], NOW + 5, "events_read_failed")
        self.assertEqual(gap["last_success_at"], NOW)
        self.assertIn('watch_up{namespace="local-rehearsal"} 0', self.monitor.metrics(NOW + 5))
        self.assertTrue(self.monitor.coverage_lost)

    def test_binding_mismatch_refuses_state_reuse(self):
        with self.assertRaisesRegex(ValueError, "watcher_state_binding_mismatch"):
            Monitor([SCOPE], NOW, "other", self.monitor.state)

    def test_malformed_response_and_corrupt_state_fail_closed(self):
        for payload in [[], None, {"kind": "EventList", "items": [], "metadata": []},
                        listing({**event(), "series": "invalid"}),
                        listing({**event(), "involvedObject": []})]:
            with self.assertRaises(ValueError):
                self.monitor.consume(SCOPE["namespace"], payload, NOW)
        for state in [[], {}, {**self.monitor.state, "seen": []},
                      {**self.monitor.state, "seen": {"broken": 1}}]:
            with self.assertRaises(ValueError):
                Monitor([SCOPE], NOW, "fixture", state)
        self.assertEqual(self.monitor.up[SCOPE["namespace"]], 0)

    def test_one_failed_namespace_does_not_shrink_expected_coverage(self):
        other = target("eg:other:cma-eg-")
        monitor = Monitor([SCOPE, other], NOW - 3600, "both")
        monitor.consume(SCOPE["namespace"], listing(), NOW)
        monitor.failure("other", NOW, "events_read_failed")
        self.assertEqual(monitor.up, {SCOPE["namespace"]: 1, "other": 0})
        self.assertIn('watch_up{namespace="other"} 0', monitor.metrics(NOW))


class ReaderTests(unittest.TestCase):
    def test_command_can_only_read_events_and_stderr_is_not_forwarded(self):
        def fake(command, **kwargs):
            self.assertEqual(command, ["kubectl", "--context", "fixture", "--namespace", "local",
                "get", "events", "--field-selector=involvedObject.kind=Pod", "--chunk-size=500",
                "--request-timeout=5s", "-o", "json"])
            self.assertEqual(kwargs["timeout"], 8)
            kwargs["stderr"].write(b"private error must not escape")
            return type("Result", (), {"returncode": 1})()
        with patch("watch_hook_events.subprocess.run", fake):
            with self.assertRaisesRegex(ValueError, "^events_read_failed$"):
                read_events("kubectl", "fixture", "local")

    def test_replay_emits_alert_metrics_and_restart_deduplicates(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            saved = root / "events.json"
            saved.write_text(json.dumps(listing(event())))
            args = parser().parse_args(["--context", "fixture", "--target", "am:local-rehearsal:cma-agent-manager-",
                                       "--output", str(root / "out"), "--replay", str(saved)])
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                self.assertEqual(run(args, clock=lambda: NOW), 1)
                self.assertEqual(run(args, clock=lambda: NOW), 0)
            lines = (root / "out/alerts.jsonl").read_text().splitlines()
            self.assertEqual(len(lines), 1)
            self.assertNotIn("command/private", output.getvalue())
            status = json.loads((root / "out/status.json").read_text())
            self.assertEqual(status["notification_delivery"], "NOT_CONFIGURED")

    def test_live_read_failure_is_nonzero_and_critical_gap(self):
        with tempfile.TemporaryDirectory() as temporary:
            args = parser().parse_args(["--context", "fixture", "--target", "eg:local:cma-eg-",
                                       "--output", temporary, "--once"])
            def broken(*_args):
                raise OSError("private endpoint must not escape")
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                self.assertEqual(run(args, reader=broken, clock=lambda: NOW), 2)
            self.assertNotIn("private endpoint", output.getvalue())
            self.assertIn("cma-hook-watch-gap-v1", output.getvalue())


if __name__ == "__main__":
    unittest.main()
