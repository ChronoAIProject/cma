"""Takeovers stop only the obsolete AM publisher and never produce a receipt."""
import contextlib
import io
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

import staging_cd_publish as publisher
from image_scanner_policy import MANIFEST_SHA256
from image_secret_gate import checksum
from staging_cd_common import Refusal
from staging_cd_history import DevelopHistory
from staging_cd_publication_diagnostics import ImageCommandRefusal
from staging_cd_receipts_tests import fixture_identity
from staging_cd_reservations import ReservationSuperseded, Reservations


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

        def build(_root, _reservation, dest, **_kwargs):
            trace.append("build")
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


if __name__ == "__main__":
    unittest.main()
