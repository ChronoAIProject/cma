#!/usr/bin/env python3
"""Sampler data safety and real Bash gate failure/lifetime regressions."""

import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
SCRIPTS = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("sampler", SCRIPTS / "backend-resource-sampler.py")
sampler = importlib.util.module_from_spec(spec)
spec.loader.exec_module(sampler)
PREFIX = "[backend-resource] "


class ResourceSamples(unittest.TestCase):
    def test_processes_are_sorted_bounded_names_only(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for pid in range(1, 13):
                process = root / str(pid)
                process.mkdir()
                (process / "comm").write_text("rustc\n::error::in-name\n" if pid == 12 else "rustc\n")
                (process / "statm").write_text(f"100 {pid} 0 0 0 0\n")
                (process / "cmdline").write_text("secret-argument-canary")
                (process / "environ").write_text("secret-environment-canary")
            (root / "99").mkdir()  # Exiting process must not lose the sample.
            rows = sampler.top_processes(root)
            self.assertEqual([row["pid"] for row in rows], list(range(12, 2, -1)))
            self.assertEqual(rows[0]["rss_kib"], 12 * os.sysconf("SC_PAGE_SIZE") // 1024)
            self.assertEqual(set(rows[0]), {"pid", "name", "rss_kib"})
            encoded = json.dumps(rows)
            self.assertNotIn("secret-", encoded)
            self.assertNotIn("\n", encoded)  # Process names cannot inject log lines.

    def test_memory_swap_and_probe_failure_are_independent(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "meminfo").write_text("MemTotal: 2048 kB\nMemAvailable: 123 kB\nSwapFree: 2 kB\nUnknown: 7 kB\n")
            (root / "vmstat").write_text("pswpin 3\npswpout 9\nother 12\n")
            record = sampler.sample(root, root)
            self.assertEqual(record["memory_kib"], {"MemTotal": 2048, "MemAvailable": 123, "SwapFree": 2})
            self.assertEqual(record["swap_pages"], {"pswpin": 3, "pswpout": 9})
            self.assertEqual(record["load"], "unavailable")
            self.assertEqual(record["cgroup_root"], "unavailable")
            self.assertIsInstance(record["disk_temp"], dict)
        with patch.object(sampler, "disk", side_effect=RuntimeError("secret-error-canary")):
            self.assertNotIn("secret-error-canary", json.dumps(sampler.sample()))

    def test_sampling_cadence_and_parent_exit(self):
        output = io.StringIO()
        with patch.object(sampler.os, "getppid", side_effect=[123, 123, 123, 1]), \
                patch.object(sampler.time, "monotonic", side_effect=[0, 0, 9.9, 10.1]), \
                patch.object(sampler.time, "sleep") as sleep, \
                patch.object(sampler, "sample", return_value={}), \
                contextlib.redirect_stdout(output):
            sampler.run(123)
        records = [json.loads(line.removeprefix(PREFIX)) for line in output.getvalue().splitlines()]
        self.assertEqual([row["step_elapsed_seconds"] for row in records], [0, 10.1])
        self.assertTrue(all(call.args == (0.2,) for call in sleep.call_args_list))


class BashGate(unittest.TestCase):
    def run_gate(self, source, python="real"):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            gate = root / "gate.sh"
            gate.write_text(source)
            env = dict(os.environ, BASH_ENV=str(SCRIPTS / "backend-resource-env.sh"))
            env.pop("CMA_RESOURCE_SAMPLER_STARTED", None)
            if python != "real":
                # Missing or crashing sampler executable must not affect Bash.
                env["PATH"] = str(root)
                if python == "crash":
                    stub = root / "python3"
                    stub.write_text("#!/bin/sh\nexit 91\n")
                    stub.chmod(0o755)
            started = time.monotonic()
            result = subprocess.run(["/bin/bash", "-e", str(gate)], env=env,
                                    capture_output=True, text=True, timeout=5)
            self.assertLess(time.monotonic() - started, 3)
            return result

    def test_sampler_failure_neither_fails_nor_hides_gate_failure(self):
        for python in ("missing", "crash", "real"):
            with self.subTest(python=python):
                result = self.run_gate("echo gate-success\n", python)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn("gate-success", result.stdout)
                result = self.run_gate("false\necho must-not-run\n", python)
                self.assertEqual(result.returncode, 1)
                self.assertNotIn("must-not-run", result.stdout)
                result = self.run_gate("exit 37\n", python)
                self.assertEqual(result.returncode, 37)

    def test_gate_traps_and_termination_remain_authoritative(self):
        result = self.run_gate("trap 'echo gate-cleanup' EXIT\nexit 42\n")
        self.assertEqual(result.returncode, 42)
        self.assertIn("gate-cleanup", result.stdout)
        result = self.run_gate("trap 'echo gate-term; exit 143' TERM\nkill -TERM $$\n")
        self.assertEqual(result.returncode, 143)
        self.assertIn("gate-term", result.stdout)
        result = self.run_gate("kill -TERM $$\n")
        self.assertEqual(result.returncode, -signal.SIGTERM)

    def test_nested_shells_do_not_duplicate_sampler_or_hold_log_open(self):
        # communicate waits for EOF: an orphan sampler holding stdout open
        # would hit the timeout even after the gate itself returned success.
        result = self.run_gate("/bin/bash -c '/bin/sleep 0.5'\necho finished\n")
        self.assertEqual(result.returncode, 0, result.stderr)
        records = [json.loads(line.removeprefix(PREFIX))
                   for line in result.stdout.splitlines() if line.startswith(PREFIX)]
        self.assertEqual(len(records), 1, result.stdout)
        self.assertIn("finished", result.stdout)
        self.assertIn("top_rss", records[0])


if __name__ == "__main__":
    unittest.main()
