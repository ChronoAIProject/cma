"""Check the ordinary-partition listing boundary without Cargo or Mongo."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import tomllib
import unittest


ROOT = Path(__file__).parent
GUARD = ROOT / "mongo-list-guard.py"


class ListGuardTests(unittest.TestCase):
    def test_partition_profile_wraps_listing_without_wrapping_test_execution(self):
        config = tomllib.loads((ROOT.parent.parent / "api/.config/nextest.toml").read_text())
        self.assertIn("wrapper-scripts", config.get("experimental", []))
        scripts = config["profile"]["mongo-ordinary"].get("scripts", [])
        self.assertEqual(len(scripts), 1, "each partition must bound its own binary discovery")
        self.assertEqual(scripts[0], {"platform": "cfg(unix)", "list-wrapper": "mongo-list"})
        wrapper = config["scripts"]["wrapper"]["mongo-list"]
        self.assertEqual(wrapper["command"], {
            "command-line": "../.github/scripts/mongo-list-guard.py",
            "relative-to": "workspace-root",
        })
        self.assertNotIn("run-wrapper", str(config))
        self.assertNotIn("scripts", config["profile"].get("default", {}))
        self.assertTrue(os.access(GUARD, os.X_OK))

    def exercise(self, scenario, arguments):
        with tempfile.TemporaryDirectory(prefix="cma-list-guard-") as directory:
            root = Path(directory)
            fixture = root / "binary.py"
            fixture.write_text('''#!/usr/bin/env python3
import json, os, pathlib, sys, time
root = pathlib.Path(__file__).parent
events = root / "calls.jsonl"
with events.open("a") as output:
    output.write(json.dumps(sys.argv[1:]) + "\\n")
attempt = len(events.read_text().splitlines())
scenario = os.environ["LIST_SCENARIO"]
if scenario == "always-hang" or (scenario == "once-hang" and attempt == 1):
    print("partial-invalid-inventory", flush=True)
    time.sleep(60)
if scenario == "error":
    print("original failure", file=sys.stderr)
    sys.exit(7)
print("synthetic_case: test\\n" * 40_000, end="")
''')
            fixture.chmod(0o755)
            worker = root / "worker.py"
            worker.write_text('''import runpy, sys
guard = runpy.run_path(sys.argv[1])
guard["listing"].__globals__["DISCOVERY_TIMEOUT_SECONDS"] = 0.25
raise SystemExit(guard["main"](sys.argv[2:]))
''')
            result = subprocess.run(
                [sys.executable, str(worker), str(GUARD), str(fixture), *arguments],
                env=dict(os.environ, LIST_SCENARIO=scenario),
                capture_output=True, text=True, timeout=5,
            )
            events = root / "calls.jsonl"
            calls = [json.loads(line) for line in events.read_text().splitlines()] if events.exists() else []
            return result, calls

    def test_complete_large_listing_preserves_each_inventory_and_argv(self):
        for extra in ([], ["--ignored"]):
            args = ["--list", "--format", "terse", *extra]
            with self.subTest(extra=extra):
                result, calls = self.exercise("complete", args)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(calls, [args])
                self.assertEqual(result.stdout, "synthetic_case: test\n" * 40_000)

    def test_stalled_listing_is_retried_without_publishing_partial_output(self):
        args = ["--list", "--format", "terse"]
        result, calls = self.exercise("once-hang", args)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(calls, [args, args])
        self.assertEqual(result.stdout, "synthetic_case: test\n" * 40_000)

    def test_repeated_stall_and_command_failure_stay_failed(self):
        args = ["--list", "--format", "terse"]
        for scenario, code, count in (("always-hang", 124, 2), ("error", 7, 1)):
            with self.subTest(scenario=scenario):
                result, calls = self.exercise(scenario, args)
                self.assertEqual(result.returncode, code, result.stderr)
                self.assertEqual(calls, [args] * count)
                self.assertEqual(result.stdout, "")
                if scenario == "error":
                    self.assertIn("original failure", result.stderr)

    def test_test_execution_or_unknown_list_arguments_are_refused_before_spawn(self):
        for args in (["--exact", "test_name"], ["--list"],
                     ["--list", "--format", "terse", "--exact", "test_name"]):
            with self.subTest(args=args):
                result, calls = self.exercise("complete", args)
                self.assertEqual(result.returncode, 2, result.stderr)
                self.assertEqual(calls, [])


if __name__ == "__main__":
    unittest.main()
