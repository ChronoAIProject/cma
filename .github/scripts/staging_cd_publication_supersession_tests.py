"""Takeovers stop only the obsolete AM publisher and never produce a receipt."""
import contextlib
import io
import json
import os
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

import staging_cd_publish as publisher
from image_scanner_policy import MANIFEST_SHA256
from image_secret_gate import checksum
from staging_cd_build_guard import run
from staging_cd_common import Refusal
from staging_cd_history import DevelopHistory
from staging_cd_publication_diagnostics import ImageCommandRefusal
from staging_cd_receipts_tests import fixture_identity
from staging_cd_reservations import ReservationSuperseded, Reservations, pending_name
from staging_cd_watermark import SupersededVersions


def reservation(version, source, owner):
    value = fixture_identity(component="am", version=f"0.13.{version}", previous=f"0.13.{version - 1}",
                             source=source, pr=owner, run_id=owner)
    del value["build"]
    return dict(value, schema="cma-version-reservation-v1")


class SupersededPublicationTests(unittest.TestCase):
    def exercise(self, version, boundary, directory):
        held = reservation(version, "a" * 40, 100)
        successor = reservation(version + 1, "b" * 40, 101)
        pending, trace = [held], []
        api = Mock()
        history = DevelopHistory(successor["source"], [successor["source"], held["source"]])
        api.history.return_value = api.history_containing.return_value = history
        api.tags.return_value = []
        owners = Reservations(api, "fixture", directory)
        owners.list = lambda component: [item for item in pending if item["component"] == component]
        release = dict(held, schema="cma-image-release-v1", build="c" * 64)
        digest = "sha256:" + "d" * 64
        registry = Mock()
        registry.manifest.side_effect = [None, (digest, {})]

        def build(_root, _reservation, dest, *, check_owner, **kwargs):
            trace.append("build")
            self.assertTrue(callable(check_owner))
            if boundary == "during-build":
                pending.append(successor)
                with patch("staging_cd_watermark.require_version_refs", side_effect=SupersededVersions({"am"})):
                    check_owner(Mock())
            if boundary == "after-build":
                pending.append(successor)
            archive = dest / "image.oci.tar"
            archive.write_bytes(b"synthetic image")
            return archive, release

        def scan(archive, report, *, component):
            self.assertEqual(component, "am")
            trace.append("scan")
            report.mkdir()
            (report / "REPORT.json").write_text(json.dumps({"policy": {"staging_runtime_component": None}, "counts": {
                "detected": 0, "blocking": 0, "advisory": 0, "reviewed": 0, "allowlisted": 0}}))
            return checksum(archive), ("1.0.4", MANIFEST_SHA256)

        def transport(args, **kwargs):
            trace.append(args[1])
            if args[1] == "login":
                Path(args[3]).touch()
                if boundary == "before-copy":
                    pending.append(successor)
            elif boundary == "copy-failed":
                pending.append(successor)
                raise ImageCommandRefusal(args, b"")

        def exists(*_):
            trace.append("verify")
            if boundary == "after-verify":
                pending.append(successor)
            return True

        registry.exists.side_effect = exists
        publish = publisher.publish

        def publish_export(*args, **kwargs):
            # Use the same public source declarations as recipe-time validation.
            return publish(*args, **dict(kwargs, source=Path(__file__).resolve().parents[2]),
                           scanner=scan, runner=transport)

        output = Path(directory, "outputs")
        receipt = Path(directory, "receipts")
        env = {"GITHUB_EVENT_NAME": "push", "GITHUB_RUN_ID": "100", "GITHUB_RUN_ATTEMPT": "1",
               "GITHUB_OUTPUT": str(output), "RUNNER_TEMP": str(directory), "GH_TOKEN": "fixture",
               "GITHUB_ACTOR": "fixture", "CD_COMPONENT": "am", "CD_RECEIPTS_PATH": str(receipt),
               "CD_TASKS": json.dumps([{"component": "am", "mode": "publish", "reservation": held}])}
        with contextlib.ExitStack() as stack:
            stack.enter_context(patch.dict(os.environ, env, clear=True))
            for name, replacement in dict(event_identity=Mock(), snapshot=Mock(), Registry=Mock(return_value=registry),
                                          GitHub=Mock(return_value=api), Reservations=Mock(return_value=owners),
                                          identity=Mock(return_value=(release, {}, {})), try_reuse=Mock(return_value=None),
                                          build=build, publish=publish_export, inspect=Mock(return_value=digest)).items():
                stack.enter_context(patch.object(publisher, name, replacement))
            log = stack.enter_context(contextlib.redirect_stdout(io.StringIO()))
            code = publisher.main()
        return code, log.getvalue(), trace, output, receipt

    def test_recorded_races_and_pre_copy_fence(self):
        for version, boundary, steps in [
            (5, "after-build", ["build"]), (6, "after-verify", ["build", "scan", "login", "copy", "verify"]),
            (7, "after-build", ["build"]), (8, "before-copy", ["build", "scan", "login"]),
            (10, "during-build", ["build"]),
            (9, "unchanged", ["build", "scan", "login", "copy", "verify"]),
        ]:
            with self.subTest(version=version, boundary=boundary), tempfile.TemporaryDirectory() as directory:
                code, log, trace, output, receipt = self.exercise(version, boundary, directory)
                self.assertEqual(code, 0, log)
                self.assertEqual(trace, steps)
                self.assertEqual(receipt.exists(), boundary == "unchanged")
                if boundary == "unchanged":
                    self.assertFalse(output.exists())
                else:
                    self.assertIn("reason=reservation_superseded", log)
                    self.assertNotIn("FAIL", log)
                    self.assertEqual(output.read_text(), "disposition=reservation_superseded\n")

    def test_registry_failure_remains_a_failure_even_if_an_owner_arrives(self):
        with tempfile.TemporaryDirectory() as directory:
            code, log, trace, output, receipt = self.exercise(6, "copy-failed", directory)
            self.assertEqual(code, 1)
            self.assertIn("FAIL publication stage=registry_copy reason=command_failed", log)
            self.assertEqual(trace, ["build", "scan", "login", "copy"])
            self.assertFalse(output.exists() or receipt.exists())

    def test_unconfirmed_takeover_or_missing_owner_cannot_be_reported_as_superseded(self):
        held = reservation(1, "a" * 40, 10)
        older = reservation(2, "b" * 40, 11)
        api = Mock()
        chain = DevelopHistory(held["source"], [held["source"], older["source"]])
        api.history.return_value = api.history_containing.return_value = chain
        api.tags.return_value = []
        owners = Reservations(api, "fixture", Path.cwd())
        for inventory in ([], [held, older]):
            owners.list = Mock(return_value=inventory)
            with self.subTest(inventory=len(inventory)), self.assertRaises(Refusal) as err:
                owners.require_current(held)
            self.assertNotIsInstance(err.exception, ReservationSuperseded)
        with patch("staging_cd_watermark.require_version_refs", side_effect=[SupersededVersions({"am"}), None]):
            owners.list = Mock(return_value=[held])
            owners.check_build_owner(held)
        with patch("staging_cd_watermark.require_version_refs", side_effect=Refusal("unreadable")):
            with self.assertRaisesRegex(Refusal, "unreadable"):
                owners.check_build_owner(held)


class GuardedBuildTests(unittest.TestCase):
    def test_persistent_inventory_disagreement_refuses_before_launch_and_during_build(self):
        held = reservation(5, "a" * 40, 100)
        successor = reservation(6, "b" * 40, 101)
        api = Mock()
        api.history.return_value = DevelopHistory(held["source"], [held["source"]])
        api.tags.return_value = []
        owners = Reservations(api, "fixture", Path.cwd())
        owners.list = Mock(return_value=[held])
        current_ref = f"{'c' * 40}\trefs/tags/{pending_name(held)}\n"
        higher_ref = f"{'d' * 40}\trefs/tags/{pending_name(successor)}\n"

        for during_build in (False, True):
            with self.subTest(during_build=during_build), tempfile.TemporaryDirectory() as directory:
                pid_file = Path(directory, "build-pid")

                def advertise(args, **kwargs):
                    conflict = not during_build or pid_file.exists()
                    return subprocess.CompletedProcess(args, 0, current_ref + (higher_ref if conflict else ""), "")

                api.runner = advertise

                command = [sys.executable, "-c", "import os,pathlib,time; "
                           "pathlib.Path('pid.tmp').write_text(str(os.getpid())); "
                           "pathlib.Path('pid.tmp').replace('build-pid'); time.sleep(20)"]
                with self.assertRaisesRegex(Refusal, "Higher component version lacks verified") as err:
                    run(command, cwd=directory, env={}, timeout=5,
                        check_owner=lambda _probe: owners.check_build_owner(held), interval=.05)
                self.assertNotIsInstance(err.exception, ReservationSuperseded)
                self.assertEqual(pid_file.exists(), during_build)
                if during_build:
                    with self.assertRaises(ProcessLookupError):
                        os.kill(int(pid_file.read_text()), 0)

    def test_supersession_reaps_build_and_helper(self):
        with tempfile.TemporaryDirectory() as directory:
            pids = Path(directory, "pids")
            script = ("import os,pathlib,subprocess,sys,time; "
                      "p=subprocess.Popen([sys.executable,'-c','import time; time.sleep(30)']); "
                      "pathlib.Path('pids').write_text(f'{os.getpid()} {p.pid}'); time.sleep(30)")

            def check(_probe):
                if pids.exists():
                    raise ReservationSuperseded()

            with self.assertRaises(ReservationSuperseded):
                run([sys.executable, "-c", script], cwd=directory, env={}, timeout=5, check_owner=check, interval=.05)
            for pid in map(int, pids.read_text().split()):
                with self.assertRaises(ProcessLookupError):
                    os.kill(pid, 0)

    def test_live_owner_preserves_command_success_failure_and_deadline(self):
        for code in (0, 7):
            result = run([sys.executable, "-c", f"import sys; sys.exit({code})"], cwd=None, env={},
                         timeout=5, check_owner=lambda _probe: None, interval=.05)
            self.assertEqual(result.returncode, code)
        with self.assertRaises(subprocess.TimeoutExpired):
            run([sys.executable, "-c", "import time; time.sleep(30)"], cwd=None, env={},
                timeout=.1, check_owner=lambda _probe: None, interval=.05)

    def test_sigterm_reaps_owned_build_group_and_preserves_sibling(self):
        with tempfile.TemporaryDirectory() as directory:
            pids = Path(directory, "pids")
            build_code = (
                "import os,pathlib,signal,subprocess,sys,time; "
                "signal.signal(signal.SIGTERM,signal.SIG_IGN); "
                "helper=subprocess.Popen([sys.executable,'-c','import time; time.sleep(30)']); "
                "pathlib.Path('pids.tmp').write_text(f'{os.getpid()} {helper.pid}'); "
                "pathlib.Path('pids.tmp').replace('pids'); time.sleep(30)"
            )
            publisher_code = (
                "import sys; from staging_cd_build_guard import run; "
                "run([sys.executable,'-c',sys.argv[1]],cwd=None,env={},timeout=15,check_owner=lambda _probe: None)"
            )
            env = {"PYTHONPATH": str(Path(__file__).resolve().parent)}
            with subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"],
                                  start_new_session=True) as sibling, subprocess.Popen(
                [sys.executable, "-B", "-c", publisher_code, build_code], env=env, cwd=directory,
            ) as publishing:
                build_pid = None
                try:
                    deadline = time.monotonic() + 5
                    while publishing.poll() is None and not pids.exists() and time.monotonic() < deadline:
                        time.sleep(.01)
                    self.assertTrue(pids.exists(), "build and helper did not start")
                    build_pid, helper_pid = map(int, pids.read_text().split())
                    publishing.terminate()
                    self.assertEqual(publishing.wait(timeout=6), -signal.SIGTERM)
                    for pid in (build_pid, helper_pid):
                        with self.assertRaises(ProcessLookupError):
                            os.kill(pid, 0)
                    self.assertIsNone(sibling.poll())
                finally:
                    if publishing.poll() is None:
                        publishing.kill()
                    if build_pid is not None:
                        with contextlib.suppress(ProcessLookupError):
                            os.killpg(build_pid, signal.SIGKILL)
                    sibling.terminate()

    def test_sigterm_while_waitpid_lock_is_acquired_reaps_the_build(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            publisher_code = """
import signal
import subprocess
import sys
from pathlib import Path
from staging_cd_build_guard import run

RealPopen = subprocess.Popen

class SignalOnAcquire:
    def __init__(self, lock):
        self.lock = lock
        self.fired = False

    def acquire(self, *args):
        acquired = self.lock.acquire(*args)
        # Inject after CPython takes the lock, before its caller can release it.
        if acquired and not self.fired and sys._getframe(1).f_code.co_name in ("_internal_poll", "_wait"):
            self.fired = True
            Path("signal-delivered").touch()
            signal.raise_signal(signal.SIGTERM)
        return acquired

    def release(self):
        self.lock.release()

    def __enter__(self):
        return self.lock.__enter__()

    def __exit__(self, *args):
        return self.lock.__exit__(*args)

class InstrumentedPopen(RealPopen):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        Path("build-pid").write_text(str(self.pid))
        self._waitpid_lock = SignalOnAcquire(self._waitpid_lock)

subprocess.Popen = InstrumentedPopen
run([sys.executable, "-c", "import time; time.sleep(20)"], cwd=None, env={}, timeout=15,
    check_owner=lambda _probe: None)
"""
            env = {"PYTHONPATH": str(Path(__file__).resolve().parent)}
            with subprocess.Popen([sys.executable, "-B", "-c", publisher_code], cwd=root, env=env) as publishing:
                try:
                    self.assertEqual(publishing.wait(timeout=8), -signal.SIGTERM)
                    self.assertTrue((root / "signal-delivered").exists())
                    with self.assertRaises(ProcessLookupError):
                        os.kill(int((root / "build-pid").read_text()), 0)
                finally:
                    if publishing.poll() is None:
                        publishing.kill()
                    pid = root / "build-pid"
                    if pid.exists():
                        with contextlib.suppress(ProcessLookupError):
                            os.killpg(int(pid.read_text()), signal.SIGKILL)

    def test_sigterm_during_supersession_cleanup_cannot_report_a_skip(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            build_code = """
import os
import signal
import time
from pathlib import Path

def acknowledge(*_):
    Path("cleanup-ack").touch()

signal.signal(signal.SIGTERM, acknowledge)
Path("pid.tmp").write_text(str(os.getpid()))
Path("pid.tmp").replace("build-pid")
time.sleep(20)
"""
            publisher_code = """
import sys
from pathlib import Path
from staging_cd_build_guard import run
from staging_cd_publication_diagnostics import run_publication
from staging_cd_reservations import ReservationSuperseded

def check_owner(_probe):
    if Path("build-pid").exists():
        raise ReservationSuperseded()

def operation(_masker, diagnostics):
    diagnostics.mark("build")
    run([sys.executable, "-c", sys.argv[1]], cwd=None, env={}, timeout=10,
        check_owner=check_owner, interval=.05)

def skip():
    Path("disposition").write_text("reservation_superseded")

raise SystemExit(run_publication(operation, on_superseded=skip))
"""
            env = {"PYTHONPATH": str(Path(__file__).resolve().parent)}
            with subprocess.Popen([sys.executable, "-B", "-c", publisher_code, build_code], cwd=root,
                                  env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT) as publishing:
                try:
                    deadline = time.monotonic() + 5
                    ack = root / "cleanup-ack"
                    while publishing.poll() is None and not ack.exists() and time.monotonic() < deadline:
                        time.sleep(.01)
                    self.assertTrue(ack.exists(), "build did not acknowledge its cleanup signal")
                    publishing.terminate()
                    output, _ = publishing.communicate(timeout=6)
                    self.assertEqual(publishing.returncode, -signal.SIGTERM, output)
                    self.assertFalse((root / "disposition").exists())
                    self.assertNotIn(b"SKIP", output)
                    with self.assertRaises(ProcessLookupError):
                        os.kill(int((root / "build-pid").read_text()), 0)
                finally:
                    if publishing.poll() is None:
                        publishing.kill()
                        pid = root / "build-pid"
                        if pid.exists():
                            with contextlib.suppress(ProcessLookupError):
                                os.killpg(int(pid.read_text()), signal.SIGKILL)
