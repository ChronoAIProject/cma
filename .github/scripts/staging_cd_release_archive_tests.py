"""Verify the reservation-bound direct handoff before publication."""

import contextlib
import io
import json
import os
import tempfile
import unittest
from datetime import date
from pathlib import Path
from unittest.mock import ANY, Mock, patch

import staging_cd_publish
import staging_cd_produce

from staging_cd_build_cargo_keys import CargoKey
from staging_cd_common import Refusal
from staging_cd_native_archive import METADATA, artifact_name
from staging_cd_release_archive import ARCHIVE, LEGACY_SCHEMA, MANIFEST, load, path_for, seal
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
            day = date(2026, 11, 25)
            seal(directory, identity, "a" * 32, day)
            handoff = load(directory, identity)
            self.assertEqual(handoff.path, archive)
            self.assertEqual(handoff.cache_namespace, "a" * 32)
            self.assertEqual(handoff.cache_day, day)
            self.assertEqual(handoff.sha256, json.loads((directory / MANIFEST).read_text())["archive_sha256"])

    def test_legacy_handoff_has_no_authority_to_reseal_cargo_for_the_publisher_day(self):
        identity = fixture_identity(component="am")
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            (directory / ARCHIVE).write_bytes(b"synthetic OCI archive")
            seal(directory, identity, "a" * 32, date(2026, 11, 25))
            manifest = directory / MANIFEST
            document = json.loads(manifest.read_text())
            manifest.write_text(json.dumps(dict(document, cache_day="2026-11-31")))
            with self.assertRaisesRegex(Refusal, "cache identity is malformed"):
                load(directory, identity)
            document.pop("cache_day")
            manifest.write_text(json.dumps(dict(document, schema=LEGACY_SCHEMA)))
            self.assertIsNone(load(directory, identity).cache_day)

    def test_normal_release_consumes_the_prepared_archive_without_rebuilding(self):
        self.check_prepared_release(overlap=True)

    def test_matching_ci_archive_skips_build_and_cache_restore(self):
        self.check_prepared_release(overlap=False, reuse=True)

    def test_low_disk_headroom_defers_export_until_verification_scratch_is_removed(self):
        self.check_prepared_release(overlap=False)

    def test_tested_native_archive_writes_the_upload_outputs(self):
        release = fixture_identity(component="am")
        reservation = {key: value for key, value in release.items() if key != "build"}
        reservation["schema"] = "cma-version-reservation-v1"
        task = {"mode": "publish", "reservation": reservation, "component": "am"}
        stamp = {"versionLabel": release["version"], "imageId": "sha256:" + "b" * 64,
                 "buildFingerprint": release["build"]}

        def build_image(_root, _reservation, directory, **_options):
            archive = directory / ARCHIVE
            archive.write_bytes(b"synthetic native archive")
            return archive, release

        def test_image(directory, _release, _source):
            (directory / METADATA).write_text(json.dumps(stamp))
            return stamp

        with tempfile.TemporaryDirectory() as temporary, contextlib.ExitStack() as stack:
            root = Path(temporary)
            output = root / "output"
            env = {"GITHUB_EVENT_NAME": "push", "GITHUB_SHA": release["source"],
                   "GITHUB_RUN_ID": str(release["run_id"]), "GITHUB_RUN_ATTEMPT": str(release["run_attempt"]),
                   "CD_COMPONENT": "am", "CD_TASKS": json.dumps([task]), "RUNNER_TEMP": str(root),
                   "CD_NATIVE_CI_PRODUCER": "true", "GITHUB_OUTPUT": str(output)}
            for name in ("event_identity", "GitHub", "snapshot", "Reservations"):
                stack.enter_context(patch.object(staging_cd_produce, name))
            stack.enter_context(patch.object(staging_cd_produce, "command",
                                             return_value=f"{release['source']}\n{release['tree']}\n".encode()))
            stack.enter_context(patch.object(staging_cd_produce, "build", side_effect=build_image))
            stack.enter_context(patch.object(staging_cd_produce, "stamp_metadata", side_effect=test_image))
            result = staging_cd_produce.produce(env, root, Mock(), {})
            self.assertEqual(result["imageId"], stamp["imageId"])
            destination = path_for(root / "cma-prepared-release", reservation)
            self.assertEqual(output.read_text(), f"native_name={artifact_name(release)}\nnative_path={destination}\n")
            self.assertIsNotNone(load(destination, release))
            self.assertTrue((destination / METADATA).is_file())

    def check_prepared_release(self, *, overlap, reuse=False):
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
                   "CD_BUILD_CACHE": "enabled", "GITHUB_OUTPUT": str(root / "output")}
            stack.enter_context(patch.dict(os.environ, env, clear=True))
            for module in (staging_cd_produce, staging_cd_publish):
                for name in ("event_identity", "GitHub", "snapshot"):
                    stack.enter_context(patch.object(module, name))
            producer_authority = stack.enter_context(patch.object(staging_cd_produce, "Reservations")).return_value
            publisher_authority = stack.enter_context(patch.object(staging_cd_publish, "Reservations")).return_value
            stack.enter_context(patch.object(staging_cd_publish, "Registry"))
            producer_day = date(2026, 11, 25)
            cache = Mock(key=CargoKey("cma-cd-cargo-v4-am-" + "e" * 64 + "-", "f" * 64), cache_day=producer_day,
                         namespace="a" * 32, restore_result="hit", export_headroom=None, export_retained_groups=None)
            cache.exports_layers.return_value = True
            cache.can_overlap_export.return_value = overlap
            export_started = Mock()
            verification_directories = []

            def finish_export():
                self.assertEqual(json.loads(Path(env["CD_RECEIPTS_PATH"]).read_text()), [receipt])
                self.assertEqual(verification_directories[0].exists(), overlap)
                return True

            @contextlib.contextmanager
            def background_export(_directory):
                export_started()
                self.assertFalse(Path(env["CD_RECEIPTS_PATH"]).exists())
                yield finish_export

            cache.background_export.side_effect = background_export
            cache.export_mounts.side_effect = finish_export
            producer_cache = stack.enter_context(patch.object(staging_cd_produce, "BuildCache", return_value=cache))
            publisher_cache = stack.enter_context(patch.object(staging_cd_publish, "BuildCache", return_value=cache))
            stack.enter_context(patch.object(staging_cd_produce, "identity", return_value=(identity, None, None)))
            stack.enter_context(patch.object(staging_cd_publish, "identity", return_value=(identity, None, None)))

            def reuse_or_miss(_release, directory, _api, _token, **options):
                self.assertTrue(options["require_reserved_label"])
                if not reuse:
                    options["on_miss"]("no-artifact")
                    return None
                archive = directory / ARCHIVE
                archive.write_bytes(b"synthetic CI OCI archive")
                return archive

            reuse_image = stack.enter_context(patch.object(staging_cd_produce, "try_reuse", side_effect=reuse_or_miss))

            def build(_root, _reservation, directory, *, progress, **_kwargs):
                progress["cache_export_error"] = "read-only"
                archive = directory / ARCHIVE
                archive.write_bytes(b"synthetic OCI candidate")
                return archive, identity

            producer_build = stack.enter_context(patch.object(staging_cd_produce, "build", side_effect=build))
            fallback = stack.enter_context(patch.object(staging_cd_publish, "build"))
            def publish_then_return(*_args, **_kwargs):
                verification_directories.append(_args[2])
                export_started.assert_not_called()
                _kwargs["on_verified_image"]()
                if overlap:
                    export_started.assert_called_once()
                else:
                    export_started.assert_not_called()
                return receipt

            publish = stack.enter_context(patch.object(staging_cd_publish, "publish", side_effect=publish_then_return))
            stack.enter_context(patch.object(staging_cd_produce.Path, "cwd", return_value=root))
            output = stack.enter_context(contextlib.redirect_stdout(io.StringIO()))
            self.assertEqual(staging_cd_produce.main(), 0)
            self.assertEqual((root / "output").read_text(), "prepared=true\n")
            producer_authority.require_current.assert_called_once_with(reservation, build_started=False)
            self.assertEqual(staging_cd_publish.main(), 0)
            prepared = load(path_for(root / "cma-prepared-release", reservation), identity)
            self.assertEqual(publish.call_args.args[0], prepared.path)
            self.assertEqual(publish.call_args.kwargs["prepared_checksum"], prepared.sha256)
            self.assertFalse(publish.call_args.kwargs["qualified"])
            self.assertIn("Staging image reuse result=prepared-reservation", output.getvalue())
            publisher_authority.require_current.assert_called_once_with(reservation, build_started=True)
            self.assertEqual(json.loads(Path(env["CD_RECEIPTS_PATH"]).read_text()), [receipt])
            fallback.assert_not_called()
            reuse_image.assert_called_once()
            if reuse:
                self.assertIn("Staging release producer result=prepared-ci-image", output.getvalue())
                self.assertIsNone(prepared.cache_day)
                producer_build.assert_not_called()
                producer_cache.assert_not_called()
                publisher_cache.assert_not_called()
                return
            self.assertIn("CI image reuse miss reason=no-artifact", output.getvalue())
            self.assertIn("build-cache-artifact=" + cache.key.family.removesuffix("-") + "\n",
                          (root / "output").read_text())
            self.assertIn("build-cache-index=graph-" + cache.key.prefix.removesuffix("-") + "\n",
                          (root / "output").read_text())
            publisher_cache.assert_called_once_with(root, ANY, ANY, "am", cache_day=producer_day)
            if overlap:
                cache.background_export.assert_called_once()
                cache.export_mounts.assert_not_called()
            else:
                cache.background_export.assert_not_called()
                cache.export_mounts.assert_called_once()
            self.assertEqual(output.getvalue().count("CACHE-SAVE cache=build-layers component=am"), 1)
            self.assertIn("result=not-saved reason=read-only", output.getvalue())
            self.assertIn("::warning title=Build cache not saved", output.getvalue())
            producer_build.assert_called_once()
            with self.assertRaisesRegex(Refusal, "destination already exists"):
                staging_cd_produce.produce(env, root, Mock(), {})
            producer_build.assert_called_once()

    def test_protected_web_prepares_raw_archive_without_ci_lookup(self):
        identity = fixture_identity(component="web")
        reservation = {
            **{key: value for key, value in identity.items() if key != "build"},
            "schema": "cma-version-reservation-v1",
        }
        task = {"mode": "publish", "reservation": reservation, "component": "web"}

        def build_raw(_root, _reservation, directory, **_options):
            archive = directory / ARCHIVE
            archive.write_bytes(b"synthetic raw Web archive")
            return archive, identity

        with tempfile.TemporaryDirectory() as temporary, contextlib.ExitStack() as stack:
            root = Path(temporary)
            env = {"GITHUB_EVENT_NAME": "push", "GITHUB_SHA": identity["source"],
                   "GITHUB_RUN_ID": str(identity["run_id"]), "GITHUB_RUN_ATTEMPT": str(identity["run_attempt"]),
                   "CD_COMPONENT": "web", "CD_TASKS": json.dumps([task]),
                   "CD_WEB_CONTINUITY_RAW": "true", "RUNNER_TEMP": str(root)}
            for name in ("event_identity", "GitHub", "snapshot", "Reservations"):
                stack.enter_context(patch.object(staging_cd_produce, name))
            reuse = stack.enter_context(patch.object(staging_cd_produce, "try_reuse"))
            build = stack.enter_context(patch.object(staging_cd_produce, "build", side_effect=build_raw))
            self.assertEqual(staging_cd_produce.produce(env, root, Mock(), {}), "prepared")
            build.assert_called_once()
            reuse.assert_not_called()

    def test_runtime_with_bounded_writer_builds_on_the_publishing_runner(self):
        for component in ("runtime", "runtime-omarchy"):
            release = fixture_identity(component=component)
            reservation = {key: value for key, value in release.items() if key != "build"}
            reservation["schema"] = "cma-version-reservation-v1"
            task = {"mode": "publish", "reservation": reservation, "component": component}

            def build_image(_root, _reservation, directory, **_options):
                archive = directory / ARCHIVE
                archive.write_bytes(b"synthetic runtime archive")
                return archive, release

            with self.subTest(component=component), tempfile.TemporaryDirectory() as temporary, \
                    contextlib.ExitStack() as stack:
                root = Path(temporary)
                env = {"GITHUB_EVENT_NAME": "push", "GITHUB_SHA": release["source"],
                       "GITHUB_RUN_ID": str(release["run_id"]), "GITHUB_RUN_ATTEMPT": str(release["run_attempt"]),
                       "CD_COMPONENT": component, "CD_TASKS": json.dumps([task]), "RUNNER_TEMP": str(root),
                       "CD_LAYER_ARCHIVE_CACHE": "enabled", "CD_LAYER_CACHE_WRITER_FLOOR": "verified"}
                for name in ("event_identity", "GitHub", "snapshot", "Reservations"):
                    stack.enter_context(patch.object(staging_cd_produce, name))
                reuse = stack.enter_context(patch.object(staging_cd_produce, "try_reuse"))
                build = stack.enter_context(patch.object(staging_cd_produce, "build", side_effect=build_image))
                self.assertEqual(staging_cd_produce.produce(env, root, Mock(), {}), "prepared")
                self.assertIsNotNone(load(path_for(root / "cma-prepared-release", reservation), release))
                build.assert_called_once()
                reuse.assert_not_called()

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
