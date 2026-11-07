#!/usr/bin/env python3
"""Sampler data safety and real Bash gate failure/lifetime regressions."""

import importlib.util
import fcntl
import json
import os
from pathlib import Path
import signal
import select
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
        with patch.object(sampler.os, "getppid", side_effect=[123, 123, 123, 1]), \
                patch.object(sampler.time, "monotonic", side_effect=[0, 0, 9.9, 10.1]), \
                patch.object(sampler.time, "sleep") as sleep, \
                patch.object(sampler, "sample", side_effect=lambda: {}), \
                patch.object(sampler, "emit_record") as emit:
            sampler.collect(123, 9)
        records = [call.args[1] for call in emit.call_args_list]
        self.assertEqual([row["step_elapsed_seconds"] for row in records], [0, 10.1])
        self.assertTrue(all(call.args == (0.2,) for call in sleep.call_args_list))

    def test_unavailable_parent_death_protection_disables_sampling(self):
        with patch.object(sampler, "arm_parent_death", return_value=False), \
                patch.object(sampler, "sample") as sample:
            sampler.run(os.getpid())
        sample.assert_not_called()


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
        self.assertEqual(len(records), 1 if sys.platform == "linux" else 0, result.stdout)
        self.assertIn("finished", result.stdout)
        if records:
            self.assertIn("top_rss", records[0])

    def test_bare_wait_does_not_wait_for_sampler(self):
        result = self.run_gate("sleep 0.05 &\nwait\necho gate-finished\nexit 39\n")
        self.assertEqual(result.returncode, 39)
        self.assertIn("gate-finished", result.stdout)


@unittest.skipUnless(sys.platform == "linux", "Linux kernel parent-death and pipe semantics")
class LinuxLifetime(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)

    def await_condition(self, predicate):
        deadline = time.monotonic() + 3
        while not predicate():
            self.assertLess(time.monotonic(), deadline, "condition never became true")
            time.sleep(0.01)

    def live(self, pid):
        try:
            # An unreaped zombie is dead and has released every descriptor.
            return Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[0] != "Z"
        except (FileNotFoundError, ProcessLookupError):
            # Exit before open yields ENOENT; exit between open/read yields
            # ESRCH. Both prove disappearance, not an inspection failure.
            return False

    def test_exit_between_stat_open_and_read_is_dead(self):
        child = subprocess.Popen(["/bin/sleep", "30"])
        self.addCleanup(child.wait, timeout=3)
        self.addCleanup(child.kill)
        original_open = Path.open

        def open_then_reap(path, *args, **kwargs):
            descriptor = original_open(path, *args, **kwargs)
            try:
                child.terminate()
                child.wait(timeout=3)
            except BaseException:
                descriptor.close()
                raise
            return descriptor

        # Keep a real proc descriptor open across exit/reaping. Linux returns
        # ESRCH on read, rather than ENOENT from opening a missing proc path.
        with patch.object(Path, "open", open_then_reap):
            self.assertFalse(self.live(child.pid))

    def test_process_inspection_preserves_liveness_and_unrelated_errors(self):
        for state, expected in [("R", True), ("S", True), ("T", True), ("Z", False)]:
            with self.subTest(state=state), \
                    patch.object(Path, "read_text", return_value=f"123 (sampler) {state} 1"):
                self.assertEqual(self.live(123), expected)
        with patch.object(Path, "read_text", side_effect=FileNotFoundError):
            self.assertFalse(self.live(123))
        for error in (PermissionError, OSError):
            with self.subTest(error=error), patch.object(Path, "read_text", side_effect=error):
                with self.assertRaises(error):
                    self.live(123)

    def launch(self, write_fd, stalled=False):
        hook = SCRIPTS / "backend-resource-env.sh"
        if stalled:
            # Keep the actual hook and lifetime/output code, but make a real
            # probe block opening a FIFO with no writer. No mock clock/signal.
            hook = self.root / hook.name
            hook.write_text((SCRIPTS / hook.name).read_text())
            fifo = self.root / "probe.fifo"
            os.mkfifo(fifo)
            (self.root / "backend-resource-sampler.py").write_text(
                "import runpy, sys\n"
                f"module = runpy.run_path({str(SCRIPTS / 'backend-resource-sampler.py')!r})\n"
                "def blocked():\n"
                f"    open({str(self.root / 'probe-entered')!r}, 'w').close()\n"
                f"    open({str(fifo)!r}, 'rb').read()\n"
                "module['collect'].__globals__['sample'] = blocked\n"
                "module['run'](int(sys.argv[1]))\n")
        environment = {**os.environ, "BASH_ENV": str(hook)}
        environment.pop("CMA_RESOURCE_SAMPLER_STARTED", None)
        # The test explicitly releases the shell only after sampler readiness.
        source = f'echo "$!" > {self.root / "pid"}\nread -r release\nexit 43\n'
        gate = subprocess.Popen(["/bin/bash", "-e", "-c", source], env=environment,
                                stdin=subprocess.PIPE, stdout=write_fd,
                                stderr=subprocess.DEVNULL, start_new_session=True)

        def reap():
            try:
                os.killpg(gate.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            gate.wait(timeout=3)
            gate.stdin.close()

        self.addCleanup(reap)
        self.await_condition(lambda: (self.root / "pid").exists()
                             and (self.root / "pid").read_text().strip())
        pid = int((self.root / "pid").read_text())
        self.await_condition(lambda: not Path(f"/proc/{pid}/fd/1").exists())
        self.assertTrue(self.live(pid), "sampler did not reach protected sampling")
        if stalled:
            self.await_condition(lambda: (self.root / "probe-entered").exists())
        return gate, pid

    def assert_pipe_eof(self, read_fd):
        deadline = time.monotonic() + 2
        while True:
            ready, _, _ = select.select([read_fd], [], [], max(0, deadline - time.monotonic()))
            self.assertTrue(ready, "orphan sampler retained the log descriptor")
            if not os.read(read_fd, 65536):
                return
            self.assertLess(time.monotonic(), deadline, "log pipe did not reach EOF")

    def assert_lifetime(self, full_pipe=False, stalled=False):
        read_fd, write_fd = os.pipe()
        try:
            if full_pipe:
                os.set_blocking(write_fd, False)
                try:
                    while True:
                        os.write(write_fd, b"x" * 4096)
                except BlockingIOError:
                    pass
                os.set_blocking(write_fd, True)
            flags = fcntl.fcntl(write_fd, fcntl.F_GETFL)
            gate, pid = self.launch(write_fd, stalled=stalled)
            self.assertEqual(fcntl.fcntl(write_fd, fcntl.F_GETFL), flags,
                             "sampler altered the gate's shared stdout flags")
            gate.stdin.write(b"release\n")
            gate.stdin.flush()
            self.assertEqual(gate.wait(timeout=2), 43)
            os.close(write_fd)
            write_fd = None
            self.await_condition(lambda: not self.live(pid))
            self.assert_pipe_eof(read_fd)
        finally:
            os.close(read_fd)
            if write_fd is not None:
                os.close(write_fd)

    def test_full_log_pipe_releases_descriptor_and_sampler_on_parent_exit(self):
        self.assert_lifetime(full_pipe=True)

    def test_stalled_probe_dies_and_releases_log_descriptor_on_parent_exit(self):
        self.assert_lifetime(stalled=True)

    def test_real_backpressure_discards_complete_record_without_changing_gate_flags(self):
        read_fd, write_fd = os.pipe()
        separate = os.open(f"/proc/self/fd/{write_fd}", os.O_WRONLY | os.O_NONBLOCK)
        try:
            while True:
                try:
                    os.write(separate, b"x" * 4096)
                except BlockingIOError:
                    break
            started = time.monotonic()
            sampler.emit_record(separate, {"test": "dropped"})
            self.assertLess(time.monotonic() - started, 1)
            self.assertFalse(fcntl.fcntl(write_fd, fcntl.F_GETFL) & os.O_NONBLOCK)
            # Drain the entire full pipe, then prove subsequent telemetry still
            # works and the dropped write left no partial JSON behind.
            os.set_blocking(read_fd, False)
            drained = b""
            while True:
                try:
                    drained += os.read(read_fd, 65536)
                except BlockingIOError:
                    break
            self.assertEqual(set(drained), {ord("x")})
            sampler.emit_record(separate, {"test": "delivered"})
            self.assertEqual(os.read(read_fd, 4096), b'[backend-resource] {"test": "delivered"}\n')
            sampler.emit_record(separate, {"oversize": "x" * 65536})
            with self.assertRaises(BlockingIOError):
                os.read(read_fd, 4096)
        finally:
            for descriptor in (read_fd, write_fd, separate):
                os.close(descriptor)


if __name__ == "__main__":
    unittest.main()
