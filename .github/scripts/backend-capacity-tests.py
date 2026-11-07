#!/usr/bin/env python3
"""Exercise capacity policy without deleting SDKs or enabling local swap."""

import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch


spec = importlib.util.spec_from_file_location(
    "capacity", Path(__file__).with_name("backend-capacity.py"))
capacity = importlib.util.module_from_spec(spec)
spec.loader.exec_module(capacity)
GIB = capacity.GIB


class CapacityTests(unittest.TestCase):
    def setUp(self):
        self.context = contextlib.ExitStack()
        self.addCleanup(self.context.close)
        self.output = io.StringIO()
        self.context.enter_context(contextlib.redirect_stdout(self.output))
        self.directory = Path(self.context.enter_context(tempfile.TemporaryDirectory()))

    def events(self):
        return [json.loads(line.split(" ", 1)[1])
                for line in self.output.getvalue().splitlines()]

    def test_only_audited_paths(self):
        self.assertEqual(capacity.CLEANUP_PATHS, (
            Path("/usr/local/lib/android"), Path("/usr/share/dotnet"),
            Path("/opt/hostedtoolcache/CodeQL")))
        self.assertFalse(capacity.safe_target(Path("/usr")))
        self.assertFalse(capacity.safe_target(Path("/opt/hostedtoolcache")))

    def test_absent_path_does_not_invoke_removal(self):
        with patch.object(capacity, "available_bytes", return_value=13 * GIB), \
                patch.object(capacity, "run") as run:
            capacity.reclaim(self.directory / "absent")
        run.assert_not_called()
        self.assertEqual(self.events()[0]["status"], "absent")
        self.assertEqual(self.events()[0]["freed_bytes"], 0)

    def test_symlink_and_symlink_ancestor_are_not_followed(self):
        sdk = self.directory / "sdk"
        sdk.mkdir()
        (sdk / "child").mkdir()
        alias = self.directory / "alias"
        alias.symlink_to(sdk, target_is_directory=True)
        with patch.object(capacity, "CLEANUP_PATHS", (alias, alias / "child")):
            self.assertFalse(capacity.safe_target(alias))
            self.assertFalse(capacity.safe_target(alias / "child"))

    def test_mount_is_not_removed(self):
        with patch.object(capacity, "CLEANUP_PATHS", (self.directory,)), \
                patch.object(Path, "is_mount", return_value=True):
            self.assertFalse(capacity.safe_target(self.directory))

    def test_removal_reports_actual_signed_free_space_delta(self):
        with patch.object(capacity, "safe_target", return_value=True), \
                patch.object(capacity, "available_bytes", side_effect=[100, 180]), \
                patch.object(capacity, "run") as run:
            capacity.reclaim(self.directory)
        run.assert_called_once_with(
            ["rm", "-rf", "--one-file-system", "--", str(self.directory)], timeout=120)
        self.assertEqual(self.events()[0]["freed_bytes"], 80)

    def test_partial_removal_failure_records_actual_reclamation(self):
        with patch.object(capacity, "safe_target", return_value=True), \
                patch.object(capacity, "available_bytes", side_effect=[100, 120]), \
                patch.object(capacity, "run", side_effect=OSError("denied")):
            capacity.reclaim(self.directory)
        self.assertEqual(self.events()[0]["status"], "OSError")
        self.assertEqual(self.events()[0]["freed_bytes"], 20)

    def test_swap_never_spends_existing_space_or_required_disk_gain(self):
        for initial in (0, 13 * GIB, 100 * GIB):
            for gain in range(-2, 50):
                size = capacity.swap_budget(initial, initial + gain * GIB)
                self.assertLessEqual(size, 8 * GIB)
                self.assertLessEqual(size, max(0, gain * GIB // 2))
                if size:
                    self.assertGreaterEqual(gain * GIB - size, 4 * GIB)
        self.assertEqual(capacity.swap_budget(13 * GIB, 33 * GIB), 8 * GIB)
        self.assertEqual(capacity.swap_budget(13 * GIB, 18 * GIB), GIB)

    def fake_swap(self):
        path = self.directory / "swap"
        self.context.enter_context(patch.object(capacity, "SWAP_PATH", path))
        # Allocate a tiny real file to exercise exclusive creation and cleanup.
        self.context.enter_context(patch.object(os, "posix_fallocate", create=True,
                                       side_effect=lambda fd, _offset, _size: os.write(fd, b"swap")))
        return path

    def test_success_keeps_private_swap_and_preserves_existing_swap(self):
        path = self.fake_swap()
        with patch.object(capacity, "run") as run:
            capacity.add_swap(8 * GIB)
            capacity.add_swap(8 * GIB)
        self.assertEqual(path.read_bytes(), b"swap")
        self.assertEqual(path.stat().st_mode & 0o777, 0o600)
        self.assertEqual([call.args[0] for call in run.call_args_list],
                         [["mkswap", str(path)], ["swapon", str(path)]])
        self.assertEqual(self.events()[-1]["status"], "existing-path-skipped")

    def test_existing_symlink_is_not_overwritten(self):
        path = self.fake_swap()
        target = self.directory / "original"
        target.write_text("original")
        path.symlink_to(target)
        with patch.object(capacity, "run") as run:
            capacity.add_swap(GIB)
        run.assert_not_called()
        self.assertEqual(target.read_text(), "original")

    def test_no_swap_for_insufficient_reclamation(self):
        with patch.object(os, "open") as create:
            capacity.add_swap(0)
        create.assert_not_called()

    def test_allocation_format_and_activation_failures_release_inactive_file(self):
        for stage in ("allocate", "format", "activate"):
            with self.subTest(stage=stage):
                path = self.fake_swap()
                effects = [None, OSError("activate")] if stage == "activate" else OSError(stage)
                with patch.object(capacity, "swap_active", return_value=False), \
                        patch.object(capacity, "run", side_effect=effects):
                    if stage == "allocate":
                        with patch.object(os, "posix_fallocate", side_effect=OSError("full")):
                            capacity.add_swap(GIB)
                    else:
                        capacity.add_swap(GIB)
                self.assertFalse(path.exists())
                self.assertEqual(self.events()[-1]["cleanup"], "removed-inactive")

    def test_uncertain_activation_never_unlinks_active_or_unknown_swap(self):
        for active in (True, OSError("unreadable")):
            path = self.fake_swap()
            with patch.object(capacity, "swap_active", side_effect=[active]), \
                    patch.object(capacity, "run", side_effect=[None, subprocess.TimeoutExpired("swapon", 30)]):
                capacity.add_swap(GIB)
            self.assertTrue(path.exists())
            path.unlink()

    def test_self_hosted_or_local_execution_is_inert(self):
        with patch.dict(os.environ, {"GITHUB_ACTIONS": "true", "RUNNER_OS": "Linux",
                                     "RUNNER_ENVIRONMENT": "self-hosted"}), \
                patch.object(capacity.sys, "platform", "linux"), \
                patch.object(os, "geteuid", return_value=0), \
                patch.object(capacity, "reclaim") as reclaim:
            self.assertEqual(capacity.main(), 0)
        reclaim.assert_not_called()

    def test_probe_failure_is_non_gating(self):
        with patch.object(capacity, "hosted_runner", return_value=True), \
                patch.object(capacity, "available_bytes", side_effect=OSError("probe")):
            self.assertEqual(capacity.main(), 0)
        self.assertEqual(self.events()[0]["event"], "warning")
        self.assertEqual(self.events()[-1]["event"], "complete")

    def test_preparation_budgets_from_measured_gain_and_logs_net_gain(self):
        with patch.object(capacity, "hosted_runner", return_value=True), \
                patch.object(capacity, "available_bytes", side_effect=[13 * GIB, 33 * GIB, 25 * GIB]), \
                patch.object(capacity, "memory_bytes", return_value={"SwapFree": 3 * GIB}), \
                patch.object(capacity, "reclaim") as reclaim, \
                patch.object(capacity, "add_swap") as add_swap:
            self.assertEqual(capacity.main(), 0)
        self.assertEqual([call.args[0] for call in reclaim.call_args_list], list(capacity.CLEANUP_PATHS))
        add_swap.assert_called_once_with(8 * GIB)
        self.assertEqual(self.events()[-2]["net_freed_bytes"], 12 * GIB)
        self.assertIn("duration_seconds", self.events()[-1])

    def test_startup_fallback_cannot_swallow_subsequent_gate_failure(self):
        # Execute the real workflow command with an unavailable sudo, then a
        # failing gate in the same fail-fast shell. No host mutation is possible.
        helper = self.directory / "sudo"
        helper.write_text("#!/bin/sh\nexit 17\n")
        helper.chmod(0o700)
        workflow = Path(__file__).parents[1] / "workflows" / "ci.yml"
        lines = workflow.read_text().splitlines()
        start = lines.index("      - name: Prepare Backend capacity") + 3
        command = " ".join(line.strip() for line in lines[start:start + 3])
        environment = {**os.environ, "PATH": str(self.directory) + os.pathsep + os.environ["PATH"]}
        result = subprocess.run(["bash", "-e", "-c", command + "\nexit 37"],
                                env=environment, capture_output=True, text=True, check=False)
        self.assertEqual(result.returncode, 37)
        self.assertIn("Backend capacity preparation unavailable", result.stdout)


if __name__ == "__main__":
    unittest.main()
