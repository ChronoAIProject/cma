"""Harness checks only; these do not execute or qualify an API image."""
import contextlib
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import repeat
import seed


def configuration():
    return {
        "environment": "isolated-local", "synthetic_identity": True,
        "origin": "http://127.0.0.1:18980", "metrics_origin": "http://127.0.0.1:18991",
        "container": "cma-memory-rca-test", "docker_image_id": "sha256:" + "a" * 64,
        "source": "67b3fbc5df5f09fcecd9ddbbba723ae010df9cc6",
    }


def event(sequence, agent="agt_test"):
    return b"event: cma.agent.event\ndata: " + json.dumps({
        "agent_id": agent, "sequence": sequence, "kind": {"content": "private-payload"}
    }).encode() + b"\n\n"


class Response(io.BytesIO):
    status = 200

    def getheader(self, name, default):
        return "text/event-stream"


class Connection:
    def __init__(self, body):
        self.response = Response(body)
        self.closed = False

    def request(self, *args, **kwargs):
        pass

    def getresponse(self):
        return self.response

    def close(self):
        self.closed = True


class DriverTests(unittest.TestCase):
    def test_fixed_twenty_minute_parameters(self):
        config = repeat.validate(configuration())
        self.assertEqual(config["rounds"] * config["round_seconds"], 1200)
        for change in ({"rounds": 16}, {"round_seconds": 59}, {"quiet_seconds": 11},
                       {"requests_per_round": True}, {"source": "develop"}):
            with self.subTest(change=change), self.assertRaises(ValueError):
                repeat.validate(configuration() | change)

    def test_loopback_private_boundary(self):
        for origin in ("https://127.0.0.1:80", "http://localhost:80", "http://127.0.0.1:80/path",
                       "http://user@127.0.0.1:80", "http://127.0.0.1:80/?x=1"):
            with self.subTest(origin=origin), self.assertRaises(ValueError):
                repeat.local_origin(origin)
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "config.json"
            path.write_text("{}")
            path.chmod(0o644)
            with self.assertRaises(ValueError):
                repeat.private_json(path)
            path.chmod(0o600)
            self.assertEqual(repeat.private_json(path), {})
            link = path.with_suffix(".link")
            link.symlink_to(path)
            with self.assertRaises(ValueError):
                repeat.private_json(link)

    def test_complete_fixed_history_closes_connection(self):
        connection = Connection(b"".join(event(i) for i in range(1, 516)))
        with patch.object(repeat, "connect", return_value=connection):
            result = repeat.read_events("http://127.0.0.1:80", "agt_test", 515, False)
        self.assertEqual(result["events"], 515)
        self.assertTrue(connection.closed and connection.response.closed)

    def test_follow_disconnects_after_first_event(self):
        connection = Connection(event(1) + b"not-consumed\n")
        with patch.object(repeat, "connect", return_value=connection):
            result = repeat.read_events("http://127.0.0.1:80", "agt_test", 515, True)
        self.assertEqual(result["events"], 1)
        self.assertTrue(connection.closed and connection.response.closed)

    def test_bad_streams_fail_and_close(self):
        for body in (event(1), event(2), event(1, "agt_other"),
                     b"event: cma.error\ndata: {}\n\n", b"event: cma.agent.event\ndata: {\n\n"):
            connection = Connection(body)
            with self.subTest(body=body[:30]), patch.object(repeat, "connect", return_value=connection):
                with self.assertRaises((ValueError, RuntimeError)):
                    repeat.read_events("http://127.0.0.1:80", "agt_test", 515, False)
            self.assertTrue(connection.closed and connection.response.closed)
        connection = Connection(b"")
        connection.response.status = 302
        with patch.object(repeat, "connect", return_value=connection), self.assertRaises(RuntimeError):
            repeat.read_events("http://127.0.0.1:80", "agt_test", 515, False)
        self.assertTrue(connection.closed)

    def test_proc_metrics_and_slope_accounting(self):
        # tail field zero is stat field 3; start ticks is tail field 19.
        stat = "1 (name with ) spaces) " + " ".join(["S"] + ["0"] * 18 + ["123"])
        fields = repeat.process_fields("VmRSS: 100 kB\nRssAnon: 70 kB\nThreads: 8\nCpus_allowed_list: 0-7\n", stat)
        self.assertEqual(fields["start_ticks"], 123)
        metrics = "\n".join(f'{name}{{kind="synthetic"}} 0' for name in repeat.METRICS)
        self.assertEqual(len(repeat.parse_metrics(metrics)), 5)
        with self.assertRaises(RuntimeError):
            repeat.parse_metrics(metrics.replace("cma_runtime_alive_tasks", "absent"))
        rows = [{"utc": str(i), "elapsed_seconds": i * 60, "rss_kib": (10 + i) * 1024} for i in range(3)]
        self.assertAlmostEqual(repeat.fit(rows)["mib_per_minute"], 1.0)
        self.assertIsNone(repeat.fit(rows[:2]))

    def exercise(self, folder, failure=False):
        config = configuration()
        dataset = Path(folder) / ("failed-fixture.json" if failure else "fixture.json")
        dataset.write_text(json.dumps({"environment": "isolated-local", "synthetic_identity": True,
                                       "agent_id": "agt_test", "head": 515, "subject": "synthetic"}))
        dataset.chmod(0o600)
        config["dataset"] = str(dataset)
        output = Path(folder) / ("failed" if failure else "complete")
        clock = [0.0]

        class Observer:
            def __init__(self, config):
                self.started = 0.0
                self.identity = ["synthetic"]

            def sample(self, phase, index):
                clock[0] += 0.001
                return {"utc": str(clock[0]), "elapsed_seconds": clock[0],
                        "process_age_seconds": clock[0], "phase": phase, "round": index,
                        "rss_kib": 10240, "threads": 8, **{name: 0 for name in repeat.METRICS}}

        def get(origin, path):
            return b'{"authenticated":true,"subject":"synthetic"}' if path == "/auth/session" else b"ok"

        def read(origin, agent, head, follow):
            if failure:
                raise RuntimeError("private-payload secret-subject must not enter artifacts")
            return {"requests": 1, "events": 1 if follow else head, "bytes": 100}

        with contextlib.ExitStack() as stack:
            stack.enter_context(patch.object(repeat, "Observer", Observer))
            stack.enter_context(patch.object(repeat, "get", get))
            stack.enter_context(patch.object(repeat, "read_events", read))
            stack.enter_context(patch.object(repeat.time, "monotonic", side_effect=lambda: clock[0]))
            stack.enter_context(patch.object(repeat.time, "sleep", side_effect=lambda seconds: clock.__setitem__(0, clock[0] + seconds)))
            if failure:
                with self.assertRaises(RuntimeError):
                    repeat.run(config, output)
            else:
                repeat.run(config, output)
        return output, json.loads((output / "result.json").read_text())

    def test_twenty_minute_counts_quiescence_and_failed_control_independence(self):
        with tempfile.TemporaryDirectory() as folder:
            failed, failure = self.exercise(folder, failure=True)
            self.assertEqual(failure["counts"]["offered"], 4)
            self.assertEqual(failure["counts"]["failed"], 4)
            self.assertEqual(failure["not_offered_reads"], 636)
            self.assertEqual(failure["twenty_minute_rss_gate"], "NOT_EVALUATED")
            self.assertNotIn("private-payload", "".join(p.read_text() for p in failed.iterdir()))
            output, success = self.exercise(folder)
            self.assertEqual(success["counts"]["finite"], 320)
            self.assertEqual(success["counts"]["follow_drop"], 320)
            self.assertEqual(success["counts"]["control"], 40)
            self.assertEqual(success["not_offered_reads"], 0)
            self.assertGreaterEqual(success["loop_seconds"], 1200)
            self.assertEqual(len(success["quiescent_accounting"]), 21)
            self.assertEqual(success["twenty_minute_rss_gate"], "PASS")
            self.assertGreaterEqual(success["full_loop_fit"]["duration_seconds"], 1200)
            self.assertIsNotNone(success["after_process_start_10_minutes_fit"])

    def test_seed_rejects_remote_or_nonfresh_scope_before_connecting(self):
        for uri, db in (("mongodb://remote:27017", "cma_memory_rca_test"),
                        ("mongodb://127.0.0.1:27017", "production"),
                        ("mongodb://127.0.0.1:27017/?replicaSet=external", "cma_memory_rca_test")):
            with self.subTest(uri=uri, db=db), self.assertRaises(ValueError):
                seed.seed(Path("nonexistent"), uri, db)


class PairTests(unittest.TestCase):
    def test_failed_first_attempt_does_not_skip_second_and_cleanup_is_owned(self):
        import pair
        calls = []

        def docker(*args, **kwargs):
            calls.append(args)
            if args[:2] == ("network", "inspect"):
                return "cma-memory-rca-fixed"
            return "ready"

        def variant(kind, *args):
            return {"outcome": "incomplete" if kind == "musl" else "completed", "variant": kind}

        with tempfile.TemporaryDirectory() as folder:
            fixture = Path(folder) / "fixture"
            fixture.write_text("{}")
            fixture.chmod(0o600)
            output = Path(folder) / "pair"
            with patch.object(pair, "docker", docker), patch.object(pair, "image", return_value={"id": "synthetic"}), \
                    patch.object(pair, "wait_primary"), patch.object(pair, "run_variant", side_effect=variant) as variants, \
                    patch.object(pair, "remove_owned") as cleanup, patch.object(pair.secrets, "token_hex", return_value="fixed"):
                pair.run(fixture, output)
            self.assertEqual([call.args[0] for call in variants.call_args_list], ["musl", "zig"])
            cleanup.assert_called_once_with("cma-memory-rca-fixed-mongo", "cma-memory-rca-fixed")
            self.assertIn(("network", "rm", "cma-memory-rca-fixed"), calls)
            result = json.loads((output / "pair.json").read_text())
            self.assertEqual(result["outcome"], "incomplete")
            self.assertEqual(result["variants"][1]["outcome"], "completed")

    def test_foreign_cleanup_refused(self):
        import pair
        with patch.object(pair, "docker", return_value="other-owner") as docker:
            with self.assertRaises(RuntimeError):
                pair.remove_owned("cma-memory-rca-test", "expected-owner")
            self.assertEqual(docker.call_count, 1)


if __name__ == "__main__":
    unittest.main()
