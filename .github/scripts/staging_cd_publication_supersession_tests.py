"""In every lane a queued reservation yields before building; a started build finishes unless a higher final exists."""
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
from staging_cd_receipts_tests import fixture_identity, fixture_receipt, fixture_tag
from staging_cd_reservations import ReservationSuperseded, Reservations
from staging_cd_targets import RUNTIME_TARGETS
from staging_cd_versions import DEPLOYABLES

PUBLISHED = ["build", "scan", "login", "copy", "verify"]
LANES = sorted(DEPLOYABLES)


def reservation(version, source, owner, component="am"):
    value = fixture_identity(component=component, version=f"0.13.{version}", previous=f"0.13.{version - 1}",
                             source=source, pr=owner, run_id=owner)
    del value["build"]
    return dict(value, schema="cma-version-reservation-v1")


def smoke(archive, identity, digest, scanned, directory, runner, *, source):
    # Runtime qualification is exercised elsewhere; only its proof shape matters here.
    if identity["component"] not in RUNTIME_TARGETS:
        return None
    return {"schema": "cma-runtime-smoke-v1", "source": identity["source"], "digest": digest, "archive": scanned,
            "evidence": "f" * 64, "scope": "local-contract-3-no-provider-or-model", "passed": True}


def final_tag(value):
    return fixture_tag(fixture_receipt(**{key: item for key, item in value.items() if key != "schema"}))


class SupersededPublicationTests(unittest.TestCase):
    def exercise(self, version, boundary, directory, *, successor="pending", component="am", pending_at=None):
        held = reservation(version, "a" * 40, 100, component)
        later = reservation(version + 1, "b" * 40, 101, component)
        newest = reservation(version + 2, "c" * 40, 102, component)
        # A contract group's earlier claim on the same source, finalized by its own build.
        peer = reservation(version - 1, held["source"], 99, component)
        pending, trace = [held], []
        api = Mock()
        history = DevelopHistory(newest["source"], [newest["source"], later["source"], held["source"]])
        api.history.return_value = api.history_containing.return_value = history
        api.tags.return_value = []
        owners = Reservations(api, "fixture", directory)
        owners.list = lambda component: [item for item in pending if item["component"] == component]
        release = dict(held, schema="cma-image-release-v1", build="c" * 64)
        digest = "sha256:" + "d" * 64
        registry = Mock()
        registry.manifest.side_effect = [None, (digest, {})]

        def arrive(point):
            # The successor is a queued reservation, a finalized newer release,
            # or the final of a same-source peer. A newer pending reservation
            # may also arrive, at its own earlier point.
            if boundary == point:
                if successor == "pending":
                    pending.append(later)
                else:
                    api.tags.return_value = [final_tag(later if successor == "final" else peer)]
            if pending_at == point:
                pending.append(newest)

        arrive("before-build")

        def build(_root, _reservation, dest, **_kwargs):
            trace.append("build")
            arrive("after-build")
            archive = dest / "image.oci.tar"
            archive.write_bytes(b"synthetic image")
            return archive, release

        def scan(archive, report, *, component):
            self.assertEqual(component, held["component"])
            trace.append("scan")
            report.mkdir()
            scope = component if component in RUNTIME_TARGETS else None
            (report / "REPORT.json").write_text(json.dumps({"policy": {"staging_runtime_component": scope}, "counts": {
                "detected": 0, "blocking": 0, "advisory": 0, "reviewed": 0, "allowlisted": 0}}))
            return checksum(archive), ("1.0.4", MANIFEST_SHA256)

        def transport(args, **kwargs):
            trace.append(args[1])
            if args[1] == "login":
                Path(args[3]).touch()
                arrive("before-copy")
            elif boundary == "copy-failed":
                pending.append(later)
                raise ImageCommandRefusal(args, b"")

        def exists(*_, settle=False):
            # Verification after the copy reads with the settle window.
            assert settle is True
            trace.append("verify")
            arrive("after-verify")
            return True

        registry.exists.side_effect = exists
        publish = publisher.publish

        def publish_export(*args, **kwargs):
            # Use the same public source declarations as recipe-time validation.
            return publish(*args, **dict(kwargs, source=Path(__file__).resolve().parents[2]),
                           scanner=scan, runner=transport, smoke=smoke)

        output = Path(directory, "outputs")
        receipt = Path(directory, "receipts")
        env = {"GITHUB_EVENT_NAME": "push", "GITHUB_RUN_ID": "100", "GITHUB_RUN_ATTEMPT": "1",
               "GITHUB_OUTPUT": str(output), "RUNNER_TEMP": str(directory), "GH_TOKEN": "fixture",
               "GITHUB_ACTOR": "fixture", "CD_COMPONENT": component, "CD_RECEIPTS_PATH": str(receipt),
               "CD_TASKS": json.dumps([{"component": component, "mode": "publish", "reservation": held}])}
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

    def test_started_build_finishes_past_pending_successor_and_queued_build_yields(self):
        for component, (version, successor, boundary, steps) in ((component, case) for component in LANES for case in [
            (5, "pending", "before-build", []), (6, "final", "before-build", []), (11, "peer", "before-build", []),
            (7, "pending", "after-build", PUBLISHED), (8, "pending", "before-copy", PUBLISHED),
            (9, "pending", "after-verify", PUBLISHED), (10, "pending", "unchanged", PUBLISHED),
        ]):
            with self.subTest(component=component, successor=successor, boundary=boundary), \
                    tempfile.TemporaryDirectory() as directory:
                code, log, trace, output, receipt = self.exercise(version, boundary, directory, successor=successor,
                                                                  component=component)
                self.assertEqual(code, 0, log)
                self.assertEqual(trace, steps)
                self.assertNotIn("FAIL", log)
                if boundary == "before-build":
                    self.assertIn("SKIP publication stage=validate reason=reservation_superseded", log)
                    # An older run's same-source final is named apart: no newer run deploys it.
                    disposition = "source_already_final" if successor == "peer" else "reservation_superseded"
                    self.assertEqual(output.read_text(), f"disposition={disposition}\n")
                    self.assertFalse(receipt.exists())
                else:
                    self.assertNotIn("SKIP", log)
                    self.assertFalse(output.exists())
                    published, = json.loads(receipt.read_text())
                    self.assertEqual(published["identity"]["version"], f"0.13.{version}")

    def test_a_final_after_build_starts_fails_without_receipt_or_skip(self):
        for component, (version, successor, pending_at, boundary, steps) in (
                (component, case) for component in LANES for case in [
                    (5, "final", None, "after-build", ["build"]),
                    (6, "final", "after-build", "before-copy", ["build", "scan", "login"]),
                    (7, "final", "after-build", "after-verify", PUBLISHED),
                    (8, "final", "before-copy", "after-verify", PUBLISHED),
                    (9, "peer", "after-build", "before-copy", ["build", "scan", "login"]),
                ]):
            with self.subTest(component=component, successor=successor, pending_at=pending_at, boundary=boundary), \
                    tempfile.TemporaryDirectory() as directory:
                # A newer pending reservation that arrives first is passed at its
                # check; only the final arriving at a later check stops the build.
                code, log, trace, output, receipt = self.exercise(version, boundary, directory, successor=successor,
                                                                  component=component, pending_at=pending_at)
                self.assertEqual(code, 1)
                self.assertEqual(trace, steps)
                self.assertIn("FAIL publication", log)
                self.assertIn("reason=validation_refused", log)
                self.assertNotIn("SKIP", log)
                self.assertFalse(output.exists() or receipt.exists())

    def test_registry_failure_remains_a_failure_even_if_an_owner_arrives(self):
        with tempfile.TemporaryDirectory() as directory:
            code, log, trace, output, receipt = self.exercise(6, "copy-failed", directory)
            self.assertEqual(code, 1)
            self.assertIn("FAIL publication stage=registry_copy reason=command_failed", log)
            self.assertEqual(trace, ["build", "scan", "login", "copy"])
            self.assertFalse(output.exists() or receipt.exists())

    def test_unconfirmed_takeover_or_missing_owner_cannot_be_reported_as_superseded(self):
        for component in LANES:
            held = reservation(1, "a" * 40, 10, component)
            older = reservation(2, "b" * 40, 11, component)
            api = Mock()
            chain = DevelopHistory(held["source"], [held["source"], older["source"]])
            api.history.return_value = api.history_containing.return_value = chain
            api.tags.return_value = []
            owners = Reservations(api, "fixture", Path.cwd())
            for started, inventory in ((False, []), (False, [held, older]), (True, [])):
                owners.list = Mock(return_value=inventory)
                with self.subTest(component=component, started=started, inventory=len(inventory)), \
                        self.assertRaises(Refusal) as err:
                    owners.require_current(held, build_started=started)
                self.assertNotIsInstance(err.exception, ReservationSuperseded)
            # Once the build has started, a higher reservation alone no longer fences it.
            owners.list = Mock(return_value=[held, older])
            self.assertTrue(owners.require_current(held, build_started=True))
            api.history_containing.assert_not_called()


if __name__ == "__main__":
    unittest.main()
