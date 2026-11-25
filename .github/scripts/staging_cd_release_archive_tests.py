"""Verify the reservation-bound direct handoff before publication."""

import contextlib
import io
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

import staging_cd_publish
import staging_cd_produce

from staging_cd_common import Refusal
from staging_cd_release_archive import ARCHIVE, MANIFEST, load, path_for, seal
from staging_cd_receipts_tests import fixture_identity, fixture_receipt


class ArchiveHandoffTests(unittest.TestCase):
    def test_handoff_returns_the_archive_and_namespaced_cache(self):
        identity = fixture_identity(component="am")
        with tempfile.TemporaryDirectory() as temporary:
            directory = path_for(Path(temporary), identity)
            self.assertIsNone(load(directory, identity))
            directory.mkdir()
            archive = directory / ARCHIVE
            archive.write_bytes(b"synthetic OCI archive")
            seal(directory, identity, "a" * 32)
            handoff = load(directory, identity)
            self.assertEqual(handoff.path, archive)
            self.assertEqual(handoff.cache_namespace, "a" * 32)
            self.assertEqual(handoff.sha256, json.loads((directory / MANIFEST).read_text())["archive_sha256"])

    def test_normal_release_consumes_the_prepared_archive_without_rebuilding(self):
        for level in ("", "0", "1"):
            with self.subTest(level=level):
                self.assert_prepared_archive_handoff(level)

    def assert_prepared_archive_handoff(self, level):
        receipt = fixture_receipt(component="am")
        identity = receipt["identity"]
        reservation = identity.copy()
        del reservation["build"]
        reservation["schema"] = "cma-version-reservation-v1"
        task = {"mode": "publish", "reservation": reservation, "component": "am"}
        with tempfile.TemporaryDirectory() as temporary, contextlib.ExitStack() as stack:
            root = Path(temporary)
            env = {"GITHUB_REPOSITORY": "ChronoAIProject/cma", "GITHUB_EVENT_NAME": "push",
                   "GITHUB_REF": "refs/heads/develop", "GITHUB_SHA": identity["source"], "GH_TOKEN": "fixture",
                   "GITHUB_RUN_ID": str(identity["run_id"]), "GITHUB_RUN_ATTEMPT": str(identity["run_attempt"]),
                   "CD_COMPONENT": "am", "CD_TASKS": json.dumps([task]), "RUNNER_TEMP": str(root),
                   "CD_RECEIPTS_PATH": str(root / "receipts.json"), "CD_PREPARED_ARCHIVE": "true",
                   "CD_BUILD_CACHE": "enabled", "GITHUB_OUTPUT": str(root / "output"),
                   "CMA_STAGING_AM_OPT_LEVEL": level}
            stack.enter_context(patch.dict(os.environ, env, clear=True))
            for module in (staging_cd_produce, staging_cd_publish):
                for name in ("event_identity", "GitHub", "snapshot"):
                    stack.enter_context(patch.object(module, name))
            producer_authority = stack.enter_context(patch.object(staging_cd_produce, "Reservations")).return_value
            publisher_authority = stack.enter_context(patch.object(staging_cd_publish, "Reservations")).return_value
            stack.enter_context(patch.object(staging_cd_publish, "Registry"))
            cache = stack.enter_context(patch.object(staging_cd_produce, "BuildCache")).return_value
            cache.key = None
            cache.component = "am"
            cache.staging_am_opt_level = level
            cache.exports_layers.return_value = True
            stack.enter_context(patch.object(staging_cd_publish, "identity", return_value=(identity, None, {"CMA_STAGING_AM_OPT_LEVEL": level} if level else {})))

            def build(_root, _reservation, directory, *, progress, **_kwargs):
                progress["cache_export_error"] = "read-only"
                archive = directory / ARCHIVE
                archive.write_bytes(b"synthetic OCI candidate")
                return archive, identity

            producer_build = stack.enter_context(patch.object(staging_cd_produce, "build", side_effect=build))
            fallback = stack.enter_context(patch.object(staging_cd_publish, "build"))
            publish = stack.enter_context(patch.object(staging_cd_publish, "publish", return_value=receipt))
            stack.enter_context(patch.object(staging_cd_produce.Path, "cwd", return_value=root))
            output = stack.enter_context(contextlib.redirect_stdout(io.StringIO()))
            self.assertEqual(staging_cd_produce.main(), 0)
            self.assertEqual((root / "output").read_text(), "prepared=true\n")
            producer_authority.require_current.assert_called_once_with(reservation, build_started=False)
            self.assertEqual(staging_cd_publish.main(), 0)
            self.assertEqual(output.getvalue().count("CACHE-SAVE cache=build-layers component=am"), 1)
            self.assertIn("result=not-saved reason=read-only", output.getvalue())
            self.assertIn("::warning title=Build cache not saved", output.getvalue())
            prepared = load(path_for(root / "cma-prepared-release", reservation), identity)
            self.assertEqual(publish.call_args.args[0], prepared.path)
            self.assertEqual(publish.call_args.kwargs["prepared_checksum"], prepared.sha256)
            self.assertIn("Staging image reuse result=prepared-reservation", output.getvalue())
            publisher_authority.require_current.assert_called_once_with(reservation, build_started=True)
            self.assertEqual(json.loads(Path(env["CD_RECEIPTS_PATH"]).read_text()), [receipt])
            fallback.assert_not_called()
            producer_build.assert_called_once()
            self.assertEqual(producer_build.call_args.kwargs["staging_am_opt_level"], level)
            with self.assertRaisesRegex(Refusal, "destination already exists"):
                staging_cd_produce.produce(env, root, Mock(), {})
            producer_build.assert_called_once()
            self.assertEqual(producer_build.call_args.kwargs["staging_am_opt_level"], level)

    def test_handoff_refuses_a_different_reservation_or_build(self):
        identity = fixture_identity(component="runtime")
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            (directory / ARCHIVE).write_bytes(b"synthetic OCI archive")
            seal(directory, identity)
            for change in ({"source": "3" * 40}, {"tree": "3" * 40}, {"build": "3" * 64},
                           {"run_id": identity["run_id"] + 1}, {"run_attempt": identity["run_attempt"] + 1}):
                with self.subTest(change=change), self.assertRaisesRegex(Refusal, "identity differs"):
                    load(directory, dict(identity, **change))

    def test_handoff_refuses_changed_bytes_or_a_symbolic_link(self):
        identity = fixture_identity(component="runtime-omarchy")
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            archive = directory / ARCHIVE
            archive.write_bytes(b"original")
            seal(directory, identity)
            archive.write_bytes(b"modified")
            with self.assertRaisesRegex(Refusal, "bytes differ"):
                load(directory, identity)
            target = directory / "other"
            archive.rename(target)
            archive.symlink_to(target)
            with self.assertRaisesRegex(Refusal, "nonregular"):
                load(directory, identity)


if __name__ == "__main__":
    unittest.main()
