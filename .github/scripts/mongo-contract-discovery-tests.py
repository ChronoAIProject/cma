"""Exercise archived discovery stalls without running Cargo or Mongo."""

import io
import json
import os
from pathlib import Path
import runpy
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from contextlib import redirect_stderr


PARTITIONS_PATH = Path(__file__).with_name("mongo-contract-partitions.py")
PARTITIONS = runpy.run_path(str(PARTITIONS_PATH))


class DiscoveryTests(unittest.TestCase):
    def exercise(self, scenario, *, interrupt=False):
        with tempfile.TemporaryDirectory(prefix="cma-discovery-") as directory:
            root = Path(directory)
            events = root / "events.jsonl"
            exceptions = json.loads(PARTITIONS_PATH.with_name("mongo-contract-exceptions.json").read_text())
            cases = {name: {"ignored": True, "filter-match": {"status": "mismatch"}}
                     for name in exceptions}
            cases.update({name: {"ignored": True, "filter-match": {"status": "matches"}}
                          for _, name in PARTITIONS["ISOLATED_TESTS"]})
            cases["synthetic::ordinary"] = {"ignored": True, "filter-match": {"status": "matches"}}
            metadata = {"rust-suites": {"cma::bin/cma": {
                "package-name": "cma", "binary-name": "cma", "kind": "bin", "testcases": cases,
            }}}
            if scenario == "large":
                metadata["synthetic-padding"] = "x" * 600_000
            (root / "inventory.json").write_text(json.dumps(metadata))
            fixture = root / "fixture.py"
            fixture.write_text('''
import json, os, pathlib, signal, subprocess, sys, time
root = pathlib.Path(__file__).parent
events = root / "events.jsonl"
def record(kind):
    with events.open("a") as output:
        output.write(json.dumps({"kind": kind, "pid": os.getpid()}) + "\\n")
def refuse_live_children():
    if not events.exists():
        return
    for line in events.read_text().splitlines():
        row = json.loads(line)
        if row["kind"] == "child":
            state = subprocess.run(["ps", "-o", "stat=", "-p", str(row["pid"])],
                                   stdout=subprocess.PIPE, text=True).stdout.strip()
            if state and not state.startswith("Z"):
                record("overlapping-discovery")
                sys.exit(22)
if sys.argv[1] == "child":
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    record("child")
    time.sleep(60)
elif sys.argv[1] == "--list":
    refuse_live_children()
    record("list")
    attempt = sum(json.loads(line)["kind"] == "list" for line in events.read_text().splitlines())
    scenario = os.environ["DISCOVERY_SCENARIO"]
    if scenario in ("always-hang", "cancel") or (scenario == "once-hang" and attempt == 1):
        subprocess.Popen([sys.executable, __file__, "child"])
        print("partial-invalid-inventory", flush=True)
        print("synthetic discovery stderr", file=sys.stderr, flush=True)
        time.sleep(60)
    elif scenario == "error":
        sys.exit(7)
    elif scenario == "malformed":
        print("invalid-json")
    else:
        print((root / "inventory.json").read_text())
else:
    refuse_live_children()
    record(sys.argv[1])
''')
            script = root / "contracts.sh"
            script.write_text('exec "$DISCOVERY_PYTHON" "$DISCOVERY_FIXTURE" "$@"\n')
            worker = root / "worker.py"
            worker.write_text('''
import os, runpy, signal, sys
module = runpy.run_path(sys.argv[1])
module["partition_plan"].__globals__["DISCOVERY_TIMEOUT_SECONDS"] = 1
if os.environ["DISCOVERY_SCENARIO"] == "cleanup-cancel":
    original_killpg = os.killpg
    def killpg(group, number):
        os.kill(os.getpid(), signal.SIGTERM)
        original_killpg(group, number)
    os.killpg = killpg
raise SystemExit(module["run"](sys.argv[2], "1/8"))
''')
            env = dict(os.environ, CMA_NEXTEST_REUSE="1", DISCOVERY_SCENARIO=scenario,
                       DISCOVERY_PYTHON=sys.executable, DISCOVERY_FIXTURE=str(fixture))
            child = subprocess.Popen([sys.executable, str(worker), str(PARTITIONS_PATH), str(script)],
                                     env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                     text=True, start_new_session=True)
            rows = []
            try:
                if interrupt:
                    deadline = time.monotonic() + 3
                    while not events.exists() or '"child"' not in events.read_text():
                        self.assertLess(time.monotonic(), deadline, "discovery child did not start")
                        time.sleep(0.01)
                    child.send_signal(signal.SIGTERM)
                try:
                    stdout, stderr = child.communicate(timeout=5)
                except subprocess.TimeoutExpired:
                    self.fail("archived discovery did not finish within its bounded recovery window")
                rows = [json.loads(line) for line in events.read_text().splitlines()]
                for row in rows:
                    state = subprocess.run(["ps", "-o", "stat=", "-p", str(row["pid"])],
                                           stdout=subprocess.PIPE, text=True, check=False).stdout.strip()
                    self.assertTrue(not state or state.startswith("Z"),
                                    f"discovery left a live {row['kind']} process: {state}")
                return child.returncode, stdout, stderr, [row["kind"] for row in rows]
            finally:
                if events.exists():
                    rows = [json.loads(line) for line in events.read_text().splitlines()]
                for pid in [row["pid"] for row in rows] + [child.pid]:
                    try:
                        os.kill(pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                child.communicate(timeout=5)

    def test_stalled_listing_is_cleaned_then_inventory_is_rediscovered_before_fixtures(self):
        code, stdout, stderr, events = self.exercise("once-hang")
        self.assertEqual(code, 0, stderr)
        self.assertEqual(events, ["list", "child", "list", "count:1/1"])
        self.assertIn("0 unclassified omissions", stdout)
        self.assertIn("synthetic discovery stderr", stderr)
        self.assertIn("discovery timed out", stderr)
        self.assertNotIn("partial-invalid-inventory", stdout)

    def test_repeated_stall_fails_without_allocating_a_fixture(self):
        code, _, stderr, events = self.exercise("always-hang")
        self.assertEqual(code, 1)
        self.assertEqual(events, ["list", "child", "list", "child"])
        self.assertIn("Cannot enumerate Mongo contracts", stderr)

    def test_command_and_inventory_errors_are_not_retried(self):
        for scenario in ("error", "malformed"):
            with self.subTest(scenario=scenario):
                code, _, stderr, events = self.exercise(scenario)
                self.assertEqual(code, 1)
                self.assertEqual(events, ["list"])
                self.assertIn("Cannot enumerate Mongo contracts", stderr)

    def test_large_complete_inventory_starts_fixtures_after_one_listing(self):
        code, stdout, stderr, events = self.exercise("large")
        self.assertEqual(code, 0, stderr)
        self.assertEqual(events, ["list", "count:1/1"])
        self.assertIn("0 unclassified omissions", stdout)

    def test_cancellation_cleans_discovery_without_retry_or_fixtures(self):
        code, _, _, events = self.exercise("cancel", interrupt=True)
        self.assertEqual(code, 128 + signal.SIGTERM)
        self.assertEqual(events, ["list", "child"])

    def test_first_cancellation_during_cleanup_cannot_start_fixtures(self):
        code, _, _, events = self.exercise("cleanup-cancel")
        self.assertEqual(code, 128 + signal.SIGTERM)
        self.assertEqual(events, ["list"])

    def test_timeout_diagnostics_only_read_owned_process_states(self):
        with tempfile.TemporaryDirectory() as directory:
            proc = Path(directory)
            for pid, group in ((101, 100), (102, 999)):
                path = proc / str(pid)
                path.mkdir()
                (path / "stat").write_text(f"{pid} (synthetic child) S 100 {group} 0")
                (path / "comm").write_text("synthetic-child\n")
                (path / "wchan").write_text("pipe_write\n")
                (path / "cmdline").write_text("private-arguments")
                (path / "environ").write_text("private-environment")
            output = io.StringIO()
            with redirect_stderr(output):
                PARTITIONS["discovery_diagnostics"](100, proc)
            report = output.getvalue()
            self.assertIn('"pid": 101', report)
            self.assertIn('"wait": "pipe_write"', report)
            for omitted in ('"pid": 102', "private-arguments", "private-environment"):
                self.assertNotIn(omitted, report)


if __name__ == "__main__":
    unittest.main()
