"""Real local child tests; no application, Mongo, provider or cluster requests."""
from pathlib import Path
import signal
import sys
import tempfile
import time
import unittest

from fixture_process import FixtureProcess, ProcessError


READY = 'print("eg_fixture status=READY database_sha256=" + "a" * 64, flush=True)'
STOPPED = 'print("eg_fixture status=STOPPED ledger_retained=true database_retained=true", flush=True)'


class FixtureProcessTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix=".fixture-process-", dir=Path(__file__).parent)
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.config = self.root / "config.json"
        self.config.write_text('{}\n'); self.config.chmod(0o600)
        self.launcher = None
        self.addCleanup(self.finish)

    def finish(self):
        if self.launcher:
            result = self.launcher.finish()
            self.assertTrue(result["fixture_process_cleanup"])

    def launcher_for(self, body, **options):
        binary = self.root / "fixture"
        binary.write_text("#!" + sys.executable + "\nimport os, signal, sys, time\n" + body + "\n")
        binary.chmod(0o700)
        self.launcher = FixtureProcess(binary, self.config, self.root / "events",
            ready_seconds=options.pop("ready_seconds", 2), lifetime_seconds=options.pop("lifetime_seconds", 5),
            shutdown_seconds=options.pop("shutdown_seconds", .3), **options)
        return self.launcher

    def ready(self, launcher):
        launcher.start()
        deadline = time.monotonic() + 2
        while not launcher.ready and time.monotonic() < deadline:
            launcher.drain()
            time.sleep(.01)
        self.assertTrue(launcher.ready)

    def test_graceful_fixture_exit_retains_config_and_redacts_unrecognized_output(self):
        launcher = self.launcher_for(
            'assert os.environ["RUST_MIN_STACK"] == "8388608"\n'
            'def stop(*_):\n    ' + STOPPED + '\n    sys.exit(0)\n'
            'signal.signal(signal.SIGTERM, stop)\n'
            'print("private-payload must not reach observations", flush=True)\n' + READY + '\n'
            'while True: time.sleep(.01)', runtime_stack_bytes=8388608)
        self.ready(launcher)
        launcher.request_stop(); self.assertEqual(launcher.poll(), "operator_stop")
        result = launcher.finish()
        self.assertEqual(result["status"], "COMPLETE")
        self.assertEqual(result["exit_code"], 0)
        self.assertTrue(result["parent_exit_observed"])
        self.assertEqual(result["gateway_process_finality"], "NOT_OBSERVED")
        self.assertEqual(result["application_acceptance"], "NOT_EVALUATED")
        self.assertTrue(self.config.exists())
        self.assertNotIn("private-payload", (launcher.output / "process-events.jsonl").read_text())
        self.assertEqual(launcher.finish(), result)

    def test_term_ignoring_stall_is_killed_without_completing_evidence(self):
        launcher = self.launcher_for('signal.signal(signal.SIGTERM, signal.SIG_IGN)\n' + READY +
                                     '\nwhile True: time.sleep(.01)', shutdown_seconds=.1)
        self.ready(launcher); launcher.request_stop(); launcher.poll()
        start = time.monotonic(); result = launcher.finish()
        self.assertLess(time.monotonic() - start, 5)
        self.assertEqual(result["status"], "INCOMPLETE")
        self.assertFalse(result["fixture_exit_within_shutdown_budget"])
        self.assertTrue(result["group_kill_sent"])
        self.assertEqual(result["exit_code"], -signal.SIGKILL)

    def test_parent_exit_does_not_hide_surviving_descendant(self):
        launcher = self.launcher_for(
            'child=os.fork()\nif child == 0:\n'
            '    signal.signal(signal.SIGTERM, signal.SIG_IGN)\n    ' + READY + '\n'
            '    while True: time.sleep(.01)\n'
            'time.sleep(.05)\nsys.exit(3)', shutdown_seconds=.1)
        self.ready(launcher)
        launcher.process.wait(timeout=2)
        self.assertTrue(launcher.group.remaining(launcher.process))
        self.assertEqual(launcher.poll(), "unexpected_fixture_exit")
        result = launcher.finish()
        self.assertEqual(result["status"], "INCOMPLETE")
        self.assertTrue(result["group_kill_sent"])
        self.assertTrue(result["fixture_process_cleanup"])
        self.assertEqual(result["exit_code"], 3)

    def test_run_enforces_lifetime_without_an_operator_stop(self):
        launcher = self.launcher_for('signal.signal(signal.SIGTERM, signal.SIG_IGN)\n' + READY +
                                     '\nwhile True: time.sleep(.01)', shutdown_seconds=.1,
                                     ready_seconds=.3, lifetime_seconds=.3)
        result = launcher.run()
        self.assertEqual(result["reason"], "lifetime_deadline")
        self.assertEqual(result["status"], "INCOMPLETE")
        self.assertTrue(result["fixture_process_cleanup"])

    def test_unterminated_ready_marker_is_not_a_ready_fixture(self):
        launcher = self.launcher_for('print("eg_fixture status=READY database_sha256=" + "a"*64, '
                                     'end="", flush=True)\nwhile True: time.sleep(.01)', ready_seconds=.2)
        result = launcher.run()
        self.assertEqual(result["reason"], "ready_deadline")
        self.assertFalse(result["ready_observed"])

    def test_overlong_line_cannot_turn_its_suffix_into_a_ready_marker(self):
        launcher = self.launcher_for('os.write(1, b"x"*8192)\ntime.sleep(.05)\n' + READY +
                                     '\nwhile True: time.sleep(.01)', ready_seconds=.2)
        result = launcher.run()
        self.assertEqual(result["reason"], "ready_deadline")
        self.assertFalse(result["ready_observed"])

    def test_config_must_be_private_regular_and_output_must_be_new(self):
        binary = Path(sys.executable)
        values = dict(ready_seconds=1, lifetime_seconds=2, shutdown_seconds=1)
        self.config.chmod(0o644)
        with self.assertRaisesRegex(ProcessError, "private_config"):
            FixtureProcess(binary, self.config, self.root / "events", **values)
        self.config.chmod(0o600)
        linked = self.root / "linked"; linked.symlink_to(self.config)
        with self.assertRaisesRegex(ProcessError, "private_config"):
            FixtureProcess(binary, linked, self.root / "events", **values)
        for field, value in [("ready_seconds", float('nan')), ("lifetime_seconds", 0),
                             ("shutdown_seconds", True), ("shutdown_seconds", 121)]:
            with self.subTest(field=field), self.assertRaises(ProcessError):
                FixtureProcess(binary, self.config, self.root / "events", **{**values, field: value})
        self.launcher = FixtureProcess(binary, self.config, self.root / "events", **values)
        with self.assertRaises(FileExistsError):
            FixtureProcess(binary, self.config, self.root / "events", **values)


if __name__ == "__main__":
    unittest.main()
