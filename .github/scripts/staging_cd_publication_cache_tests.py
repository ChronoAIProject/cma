"""Protected cache writers cannot bypass fresh image publication gates."""
import io
import contextlib
from datetime import date
import json
import os
import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

from staging_cd_build_cache import BuildCache, SCOPES, bridge, identity, layer_options, prepare
from staging_cd_build_progress import summarize
from staging_cd_common import Refusal
from staging_cd_cache_context import context
from staging_cd_timing import Timings

ROOT = Path(__file__).resolve().parents[2]


class CacheTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.env = {"GITHUB_REPOSITORY": "ChronoAIProject/cma", "GITHUB_EVENT_NAME": "push",
                    "GITHUB_REF": "refs/heads/develop", "GITHUB_SHA": "a" * 40,
                    "GITHUB_RUN_ID": "123", "GITHUB_RUN_ATTEMPT": "1", "CD_COMPONENT": "eg",
                    "RUNNER_TEMP": str(self.root), "CD_TASKS": json.dumps([{"component": "eg", "mode": "resume"}])}

    def cache(self, *, day=None):
        return BuildCache(ROOT, self.env, ROOT, "eg", cache_day=day)

    def populate(self, directory, target_member="release/incremental/synthetic/work-products.bin"):
        directory.mkdir()
        for name in ('registry', 'target'):
            with tarfile.open(directory / (name + '.tar.gz'), 'w:gz') as archive:
                member = tarfile.TarInfo(target_member if name == 'target' else 'synthetic-data')
                member.size = 4
                archive.addfile(member, io.BytesIO(b'data'))

    def test_rust_service_compacted_bundle_is_sealed_and_restores_without_the_marker(self):
        import staging_cd_build_cache as module

        tool = bridge()

        def transfer(operation, directory, **kwargs):
            self.assertEqual(operation, "extract")
            self.assertTrue(kwargs["compact_registry"])
            self.assertEqual(kwargs["max_archive"], module.MAX_BUNDLE_BYTES)
            self.populate(directory)
            (directory / "compacted").write_text("2\n")

        for component in ("am", "eg"):
            with self.subTest(component=component):
                writer_env = dict(self.env, CD_COMPONENT=component)
                cache = BuildCache(ROOT, writer_env, ROOT, component)
                with patch.object(module, "bridge", return_value=tool), patch.object(tool, "transfer", side_effect=transfer):
                    self.assertTrue(cache.export_mounts())
                self.assertEqual(cache.export_retained_groups, 2)
                self.assertFalse((cache.accepted / "compacted").exists())
                tool.validate_bundle(cache.accepted, cache.key.prefix, family=cache.key.family)

    def test_background_export_keeps_the_previous_seed_until_accepted(self):
        cache = self.cache()
        self.populate(cache.accepted)
        tool = bridge()
        tool.seal(cache.accepted, cache.key.prefix)
        before = (cache.accepted / "manifest.json").read_bytes()
        process = Mock()
        process.wait.return_value = 0
        process.poll.return_value = 0

        def start(candidate, _context, **kwargs):
            self.assertEqual(kwargs["namespace"], cache.namespace)
            self.populate(candidate, target_member="release/incremental/new-seed/work-products.bin")
            return process

        with patch("staging_cd_build_cache.bridge", return_value=tool), \
                patch.object(tool, "start_extract", side_effect=start):
            with cache.background_export(self.root) as finish:
                process.wait.assert_not_called()
                self.assertEqual((cache.accepted / "manifest.json").read_bytes(), before)
                self.assertTrue(finish())
        self.assertNotEqual((cache.accepted / "manifest.json").read_bytes(), before)
        tool.validate_bundle(cache.accepted, cache.key.prefix, family=cache.key.family)

    def test_rejected_publication_terminates_the_running_export(self):
        cache = self.cache()
        tool = bridge()
        with subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"],
                              start_new_session=True) as process:
            try:
                with patch("staging_cd_build_cache.bridge", return_value=tool), \
                        patch.object(tool, "start_extract", return_value=process):
                    with self.assertRaisesRegex(Refusal, "fixture registry refused"):
                        with cache.background_export(self.root):
                            self.assertIsNone(process.poll())
                            raise Refusal("fixture registry refused")
                self.assertIsNotNone(process.poll())
                self.assertFalse(cache.accepted.exists())
            finally:
                if process.poll() is None:
                    process.kill()

    def test_background_export_deducts_time_already_spent_publishing(self):
        cache = self.cache()
        tool = bridge()
        process = Mock()
        process.wait.side_effect = subprocess.TimeoutExpired("buildx", 80)
        process.poll.return_value = None
        with patch("staging_cd_build_cache.bridge", return_value=tool), \
                patch.object(tool, "start_extract", return_value=process), \
                patch("staging_cd_build_cache.time.monotonic", side_effect=[100, 380]), \
                patch("staging_cd_build_cache.terminate") as terminate:
            with cache.background_export(self.root) as finish:
                self.assertFalse(finish())
        process.wait.assert_called_once_with(timeout=80)
        terminate.assert_called_once_with(process, grace_seconds=0)
        self.assertEqual(cache.export_reason, "transfer-failed")
        self.assertFalse(cache.accepted.exists())

    def test_cache_output_failure_is_reported_as_withheld(self):
        from staging_cd_publish import record_cache_export

        cache = self.cache()
        metrics = []
        record_cache_export(cache, True, dict(self.env, GITHUB_OUTPUT=str(self.root)), metrics)
        self.assertEqual(metrics, [{"operation": "export", "result": "withheld", "reason": "invalid"}])

    def test_overlap_leaves_room_for_the_cache_and_registry_copies(self):
        from staging_cd_build_cache import MAX_BUNDLE_BYTES

        cache = self.cache()
        archive = self.root / "image.tar"
        archive.write_bytes(b"fixture")
        required = 7 + 3 * MAX_BUNDLE_BYTES
        with patch("staging_cd_build_cache.shutil.disk_usage", return_value=Mock(free=required - 1)) as disk:
            self.assertFalse(cache.can_overlap_export(archive))
            disk.return_value.free = required
            self.assertTrue(cache.can_overlap_export(archive))
        self.assertFalse(cache.can_overlap_export(self.root / "missing-image.tar"))

    def test_legacy_layer_cache_is_read_only(self):
        for event, ref in [('push', 'refs/heads/develop'),
                           ('push', 'refs/heads/main'),
                           ('push', 'refs/heads/topic'),
                           ('pull_request', 'refs/pull/12/merge'),
                           ('workflow_dispatch', 'refs/heads/develop'),
                           ('pull_request_target', 'refs/heads/develop')]:
            args = layer_options(dict(self.env, GITHUB_EVENT_NAME=event, GITHUB_REF=ref), 'eg')
            self.assertIn('--cache-from', args)
            self.assertNotIn('--cache-to', args)
            self.assertFalse(any('mode=max' in arg for arg in args))
            values = [arg for arg in args if not arg.startswith('--')]
            self.assertEqual(values[0], 'type=gha,version=2,scope=cma-cd-eg,timeout=2m')
            self.assertEqual(values[1], 'type=gha,version=2,scope=cma-eg,timeout=2m')
            self.assertEqual(len(values), 2)
        self.assertEqual(len(set(SCOPES.values())), 5)
        for component, scope in SCOPES.items():
            self.assertIn('scope=' + scope + ',', layer_options(self.env, component)[1])
        for env, component in [(dict(self.env, GITHUB_REPOSITORY='fork/repo'), 'eg'), (self.env, 'unknown')]:
            with self.assertRaises(Refusal):
                layer_options(env, component)

    def test_cd_and_ci_preserve_legacy_component_restore_scopes(self):
        from staging_cd_build_cache import CI_SCOPES
        self.assertFalse(set(SCOPES.values()) & CI_SCOPES)
        for component in SCOPES:
            with self.subTest(component=component):
                args = layer_options(dict(self.env, GITHUB_EVENT_NAME="push", GITHUB_REF="refs/heads/develop"), component)
                options = list(zip(args[::2], args[1::2]))
                exports = [value for flag, value in options if flag == "--cache-to"]
                self.assertEqual(exports, [])
        ci = (Path(__file__).resolve().parents[1] / "workflows" / "ci-images.yml").read_text()
        for scope in ("cma-am", "cma-eg", "cma-web"):
            self.assertIn("'" + scope + "'", ci)

    def test_runtime_builds_read_legacy_shared_stages(self):
        for component, scopes in (("runtime", ["cma-runtime", "runtime"]),
                                  ("runtime-omarchy", ["cma-runtime-omarchy", "cma-runtime", "runtime"])):
            for event in ("push", "pull_request"):
                with self.subTest(component=component, event=event):
                    args = layer_options(dict(self.env, GITHUB_EVENT_NAME=event), component)
                    options = list(zip(args[::2], args[1::2]))
                    imports = [value for flag, value in options if flag == "--cache-from"]
                    exports = [value for flag, value in options if flag == "--cache-to"]
                    self.assertEqual(imports, ["type=gha,version=2,scope=" + scope + ",timeout=2m"
                                               for scope in scopes])
                    self.assertEqual(exports, [])

    def test_local_layer_archive_is_opt_in_and_exports_to_a_fresh_directory(self):
        cache = self.cache()
        self.assertNotIn('--cache-to', cache.build_options())
        env = dict(self.env, CD_LAYER_ARCHIVE_CACHE='enabled')
        cache = BuildCache(ROOT, env, ROOT, 'eg')
        restored = cache.layer_root / 'restored'
        pending = cache.layer_root / 'pending'
        restored.mkdir(parents=True)
        (restored / 'index.json').write_text('interrupted validation')
        pending.mkdir()
        (pending / 'old').write_text('previous build')
        self.assertNotIn('type=local,src=' + str(restored), cache.build_options())
        self.assertNotIn('--cache-to', cache.build_options())
        self.assertTrue(pending.exists())
        cache.env = dict(env, CD_LAYER_CACHE_WRITER_FLOOR='verified')
        self.assertNotIn('type=local,src=' + str(restored), cache.build_options())
        self.assertFalse(pending.exists())
        cache.env = dict(cache.env, CD_LAYER_ARCHIVE_RESTORED='true')
        options = cache.build_options()
        self.assertIn('type=local,src=' + str(restored), options)
        self.assertIn('type=local,mode=min,image-manifest=true,oci-mediatypes=true,ignore-error=true,dest=' + str(pending), options)
        self.assertFalse(pending.exists())
        self.assertFalse(cache.exports_layers())

    def test_legacy_drain_keeps_cargo_and_validated_local_layer_cache(self):
        env = dict(self.env, CD_LEGACY_BUILDKIT_DRAIN='enabled')
        cache = BuildCache(ROOT, env, ROOT, 'eg')
        mount = 'BUILDKIT_CACHE_MOUNT_NS=' + cache.namespace
        self.assertEqual(cache.build_options(), ['--build-arg', mount])

        restored = cache.layer_root / 'restored'
        restored.mkdir(parents=True)
        cache.env = dict(env, CD_LAYER_ARCHIVE_CACHE='enabled', CD_LAYER_ARCHIVE_RESTORED='true')
        self.assertEqual(cache.build_options(), ['--cache-from', 'type=local,src=' + str(restored),
                                                '--build-arg', mount])

    def test_am_publication_does_not_export_optional_dependency_layers(self):
        env = dict(self.env, CD_COMPONENT='am', CD_LAYER_ARCHIVE_CACHE='enabled',
                   CD_LAYER_CACHE_WRITER_FLOOR='verified')
        cache = BuildCache(ROOT, env, ROOT, 'am')
        self.assertNotIn('--cache-to', cache.build_options())
        self.assertFalse(cache.exports_layers())

    def test_only_develop_producers_can_prepare_or_transfer_cargo_caches(self):
        for key, value in (("GITHUB_EVENT_NAME", "pull_request"), ("GITHUB_EVENT_NAME", "workflow_dispatch"),
                           ("GITHUB_REF", "refs/pull/12/merge"), ("GITHUB_REPOSITORY", "fork/repo"),
                           ("CD_COMPONENT", "unrecognized"), ("GITHUB_RUN_ID", "bad")):
            with self.subTest(key=key), self.assertRaises(Refusal):
                context({**self.env, key: value})
        self.assertIn('cma-cd-cargo-v4-eg-', prepare(ROOT, self.env)['prefix'])

    def test_lookup_and_save_key_follow_reserved_source(self):
        from staging_cd_reservations import next_reservation
        reservation = next_reservation('eg', 'c' * 40, 'd' * 40, 17, 'patch', 19, 1, [], [])
        task = {'component': 'eg', 'mode': 'publish', 'reservation': reservation}
        env = dict(self.env, CD_TASKS=json.dumps([task]))
        actual = identity(ROOT, 'eg')
        with patch('staging_cd_build_cache.snapshot') as snapshot, \
                patch('staging_cd_build_cache.identity', return_value=actual) as key:
            self.assertEqual(prepare(ROOT, env)['key'], actual.archive_key(env))
            source = snapshot.call_args.args[2]
            snapshot.assert_called_once_with(ROOT, reservation["source"], source)
            key.assert_called_once_with(source, 'eg')
        task['reservation'] = dict(reservation, source='invalid')
        with self.assertRaises(Refusal):
            prepare(ROOT, dict(env, CD_TASKS=json.dumps([task])))

    def test_staging_bundles_use_the_artifact_bound_and_report_why_exports_are_withheld(self):
        import staging_cd_build_cache as module
        import staging_cd_cargo_artifact as transport
        self.assertEqual(transport.MAX_BUNDLE_BYTES, 2 * 1024 ** 3)
        self.assertEqual(transport.MAX_EXPANDED_BYTES, 6 * 1024 ** 3)
        self.assertEqual(transport.MAX_ARTIFACT_BYTES, transport.MAX_BUNDLE_BYTES + 4096 + 1024 ** 2)
        self.assertEqual((transport.DOWNLOAD_SECONDS, transport.DEADLINE_SECONDS, module.TRANSFER_SECONDS),
                         (360, 420, 360))
        cache = self.cache()
        self.populate(cache.accepted)
        tool = bridge()
        tool.seal(cache.accepted, cache.key.prefix)
        size = sum((cache.accepted / f"{name}.tar.gz").stat().st_size for name in ("registry", "target"))
        # Larger than the Actions cache bound, within the artifact bound.
        with patch.object(tool, 'transfer') as transfer, patch('staging_cd_build_cache.bridge', return_value=tool), \
                patch.object(tool, 'MAX_ARCHIVE_BYTES', size - 1):
            with patch.object(module, 'MAX_BUNDLE_BYTES', size):
                self.assertTrue(cache.restore())
                self.assertEqual(cache.restore_result, "hit")
                transfer.side_effect = lambda op, directory, **kwargs: self.populate(directory)
                self.assertTrue(cache.export_mounts())
                self.assertIsNone(cache.export_reason)
                self.assertEqual(cache.export_headroom, "low")
                for call in transfer.call_args_list:
                    self.assertEqual((call.kwargs["timeout"], call.kwargs["max_content"]),
                                     (module.TRANSFER_SECONDS, module.MAX_EXPANDED_BYTES))
            with patch.object(module, 'MAX_BUNDLE_BYTES', 100 * size):
                self.assertTrue(cache.export_mounts())
                self.assertEqual(cache.export_headroom, "ok")
            with patch.object(module, 'MAX_BUNDLE_BYTES', size - 1):
                self.assertFalse(cache.export_mounts())
                self.assertEqual(cache.export_reason, "oversize-compressed")
            self.assertEqual(tool.validate_bundle(cache.accepted, cache.key.prefix, max_archive=size)["prefix"],
                             cache.key.prefix)
            transfer.side_effect = subprocess.TimeoutExpired("buildx", 150)
            self.assertFalse(cache.export_mounts())
            self.assertEqual(cache.export_reason, "transfer-failed")
            transfer.side_effect = lambda op, directory, **kwargs: (self.populate(directory),
                                                                    (directory / 'oversize').touch())
            self.assertFalse(cache.export_mounts())
            self.assertEqual(cache.export_reason, "oversize-expanded")
        self.assertEqual(module.withheld_reason(ValueError("unexpected cache files")), "invalid")
        from staging_cd_publish import emit_cache_metrics
        for metric, warning in (({"operation": "export", "result": "accepted"}, None),
                                ({"operation": "export", "result": "accepted", "headroom": "low"}, "above 75% of"),
                                ({"operation": "export", "result": "withheld", "reason": "oversize-compressed"},
                                 "WITHHELD build-cache reason=oversize-compressed component=am cause=change"),
                                ({"operation": "export", "result": "withheld", "reason": "transfer-failed"},
                                 "WITHHELD build-cache reason=transfer-failed component=am cause=transient"),
                                ({"operation": "restore", "result": "hit"}, None)):
            with self.subTest(metric=metric):
                stream = io.StringIO()
                with patch.dict(os.environ, {}, clear=True):
                    emit_cache_metrics("am", [metric], stream)
                lines = stream.getvalue().splitlines()
                self.assertEqual(json.loads(lines[0]), {"event": "publication-cache", **metric})
                if warning is None:
                    self.assertEqual(len(lines), 1)
                elif warning.startswith("WITHHELD "):
                    # Every withheld export explains its reason and next step.
                    self.assertEqual(lines[1], warning)
                    self.assertTrue(lines[2].startswith("::warning title=build-cache%3A "))
                    self.assertIn("Next step: ", lines[2])
                    self.assertEqual(len(lines), 3)
                else:
                    self.assertEqual(lines[1], "::warning title=Cargo cache bound::am Cargo bundle is "
                                     f"{warning} its bound in staging_cd_cargo_artifact.py")
        self.assertEqual(module.withheld_reason(OSError("disk")), "invalid")

    def test_validated_mounts_restore_and_corrupt_content_builds_cold(self):
        cache = self.cache()
        self.assertFalse(cache.restore())
        self.populate(cache.accepted)
        tool = bridge()
        tool.seal(cache.accepted, cache.key.prefix)
        with patch.object(tool, 'transfer') as transfer, patch('staging_cd_build_cache.bridge', return_value=tool):
            self.assertTrue(cache.restore())
            transfer.assert_called_once_with('inject', cache.accepted, namespace=cache.namespace,
                                             timeout=360, max_content=6 * 1024 ** 3)
            # A hit now refreshes the bundle; an empty extraction is withheld and
            # leaves the accepted bundle untouched.
            self.assertFalse(cache.export_mounts())
            self.assertEqual(transfer.call_args.args[0], 'extract')
            self.assertEqual(tool.validate_bundle(cache.accepted, cache.key.prefix)["prefix"], cache.key.prefix)
            (cache.accepted / 'target.tar.gz').write_bytes(b'changed')
            self.assertFalse(self.cache().restore())
            self.assertEqual(transfer.call_count, 2)

    def test_yesterdays_bundle_seeds_only_incremental_state_after_midnight(self):
        tool = bridge()
        yesterday, today = date(2026, 11, 20), date(2026, 11, 21)
        producer = self.cache(day=yesterday)
        self.populate(producer.accepted)
        tool.seal(producer.accepted, producer.key.prefix, family=producer.key.family)
        cache = self.cache(day=today)
        self.assertEqual(self.cache(day=producer.cache_day).key, producer.key)
        self.assertEqual(cache.key.family, producer.key.family)
        self.assertNotEqual(cache.key.prefix, producer.key.prefix)
        with patch.object(tool, 'transfer') as transfer, patch('staging_cd_build_cache.bridge', return_value=tool):
            self.assertTrue(cache.restore())
        self.assertEqual(cache.restore_result, "partial")
        # The registry and every non-incremental target member stay behind.
        transfer.assert_called_once_with("inject-target", cache.accepted, namespace=cache.namespace,
                                         timeout=360, max_content=6 * 1024 ** 3)
        shutil = __import__('shutil')
        shutil.rmtree(producer.accepted)
        self.populate(producer.accepted, target_member="release/deps/libsynthetic.rlib")
        tool.seal(producer.accepted, producer.key.prefix)
        with patch.object(tool, 'transfer') as transfer, patch('staging_cd_build_cache.bridge', return_value=tool):
            self.assertFalse(cache.restore())
        transfer.assert_not_called()
        self.assertEqual(cache.restore_result, "miss")

    def test_partial_restore_seeds_current_graph_only_after_qualification(self):
        cache = self.cache()
        self.populate(cache.accepted)
        tool = bridge()
        old = cache.key.family + "b" * 64 + "-"
        tool.seal(cache.accepted, old)
        with patch.object(tool, 'transfer') as transfer, patch('staging_cd_build_cache.bridge', return_value=tool):
            self.assertTrue(cache.restore())
            self.assertEqual(cache.restore_result, "partial")
            transfer.assert_called_once_with("inject-target", cache.accepted, namespace=cache.namespace,
                                             timeout=360, max_content=6 * 1024 ** 3)
            transfer.side_effect = lambda op, directory, **kwargs: self.populate(directory)
            self.assertTrue(cache.export_mounts())
            self.assertEqual(tool.validate_bundle(cache.accepted, cache.key.prefix)["prefix"], cache.key.prefix)
            transfer.side_effect = None
            self.assertTrue(cache.restore())
            self.assertEqual(cache.restore_result, "hit")
            # An exact hit still exports the newer compiler state for the next build.
            transfer.side_effect = lambda op, directory, **kwargs: self.populate(directory)
            self.assertTrue(cache.export_mounts())
            self.assertEqual(tool.validate_bundle(cache.accepted, cache.key.prefix)["prefix"], cache.key.prefix)

    def test_family_miss_and_failed_transfer_use_clean_build_namespaces(self):
        first = self.cache()
        self.populate(first.accepted)
        tool = bridge()
        tool.seal(first.accepted, first.key.prefix)
        with patch('staging_cd_build_cache.bridge', return_value=tool), patch.object(tool, 'transfer') as transfer:
            self.assertTrue(first.restore())
            second = self.cache()
            second.key = type(first.key)("cma-cd-cargo-v4-eg-other-", first.key.graph)
            self.assertFalse(second.restore())
            self.assertNotEqual(first.namespace, second.namespace)
            self.assertIn("BUILDKIT_CACHE_MOUNT_NS=" + second.namespace, second.build_options())
            self.assertEqual(transfer.call_count, 1)
            broken = self.cache()
            transfer.side_effect = subprocess.CalledProcessError(1, "synthetic transfer")
            self.assertFalse(broken.restore())
            abandoned = transfer.call_args.kwargs["namespace"]
            self.assertNotIn(abandoned, {first.namespace, second.namespace, broken.namespace})
            self.assertIn("BUILDKIT_CACHE_MOUNT_NS=" + broken.namespace, broken.build_options())

    def test_fallback_rejects_other_families_legacy_and_untrimmed_targets(self):
        cache = self.cache()
        self.populate(cache.accepted, "release/synthetic-binary")
        tool = bridge()
        with patch.object(tool, 'transfer') as transfer, patch('staging_cd_build_cache.bridge', return_value=tool):
            for old in (cache.key.family + "b" * 64 + "-", cache.key.family + "short-",
                        cache.key.prefix.replace("-eg-", "-am-"), cache.key.prefix.replace("-v4-", "-v3-"),
                        "cma-cd-cargo-v2-eg-legacy"):
                tool.seal(cache.accepted, old)
                self.assertFalse(cache.restore())
                self.assertEqual(cache.restore_result, "miss")
            transfer.assert_not_called()

    def test_malformed_or_linked_cache_never_imports(self):
        cache = self.cache()
        cache.accepted.mkdir()
        for value in (None, [{}], {'format': 2}, {'archives': None}):
            (cache.accepted / 'manifest.json').write_text(json.dumps(value))
            self.assertFalse(self.cache().restore())
        outside = self.root / 'outside'
        cache.accepted.rename(outside)
        cache.accepted.symlink_to(outside, target_is_directory=True)
        self.assertFalse(self.cache().restore())

    def test_export_failure_never_becomes_ready_and_success_validates_bundle(self):
        cache = self.cache()
        tool = bridge()
        with patch.object(tool, 'transfer', side_effect=subprocess.TimeoutExpired('synthetic', 1)), \
                patch('staging_cd_build_cache.bridge', return_value=tool):
            self.assertFalse(cache.export_mounts())
            self.assertFalse(cache.accepted.exists())
        with patch.object(tool, 'transfer', side_effect=lambda operation, directory, **kwargs: self.populate(directory)), \
                patch('staging_cd_build_cache.bridge', return_value=tool):
            self.assertTrue(cache.export_mounts())
            tool.validate_bundle(cache.accepted, cache.key.prefix)
            self.assertFalse(cache.pending.exists())

    def test_cache_credentials_are_only_passed_to_the_buildx_client(self):
        isolated = self.cache()
        self.assertEqual(isolated.build_options(), ["--build-arg", "BUILDKIT_CACHE_MOUNT_NS=" + isolated.namespace])
        self.assertEqual(self.cache().build_environment(), {})
        self.env.update(ACTIONS_RUNTIME_TOKEN='synthetic-cache-runtime', ACTIONS_RESULTS_URL='https://example.invalid',
                        GH_TOKEN='synthetic-registry-credential', UNRELATED_SECRET='synthetic')
        cache = self.cache()
        self.assertEqual(cache.build_environment(), {key: self.env[key] for key in ('ACTIONS_RUNTIME_TOKEN', 'ACTIONS_RESULTS_URL')})
        self.assertNotIn('synthetic', ' '.join(cache.build_options()))
        from staging_cd_build import command
        with patch.dict(os.environ, self.env), patch('staging_cd_build.subprocess.run',
                return_value=Mock(returncode=0, stdout=b'', stderr=b'')) as run:
            command(['skopeo', 'inspect', 'synthetic'])
            self.assertNotIn('ACTIONS_RUNTIME_TOKEN', run.call_args.kwargs['env'])
            self.assertNotIn('GH_TOKEN', run.call_args.kwargs['env'])

    def test_warm_cache_still_builds_once_and_scan_refusal_stops_publication(self):
        for result in ("hit", "partial"):
            with self.subTest(result=result):
                self.check_warm_scan_refusal(result)

    def check_warm_scan_refusal(self, result):
        import staging_cd_publish as publication
        from image_secret_gate import ScanRefusal
        from staging_cd_receipts_tests import fixture_identity
        import staging_cd_publish_tests as fixtures
        release = fixture_identity()
        reservation = {key: value for key, value in release.items() if key != 'build'}
        reservation['schema'] = 'cma-version-reservation-v1'
        env = dict(self.env, CD_COMPONENT='web', GITHUB_RUN_ID='100', CD_BUILD_CACHE='enabled',
                   GH_TOKEN='synthetic', GITHUB_ACTOR='synthetic',
                   CD_RECEIPTS_PATH=str(self.root / 'receipts.json'),
                   CD_TASKS=json.dumps([{'component': 'web', 'mode': 'publish', 'reservation': reservation}]))
        archive, digest, registry, _, runner = fixtures.PublicationTests().fixture(self.root)
        cache = Mock(restore_result=result)
        cache.restore.return_value = True
        finish_export = Mock()
        cache.background_export.return_value = contextlib.nullcontext(finish_export)
        scanner = Mock(side_effect=ScanRefusal('synthetic refusal'))
        real_publish = publication.publish
        def publish(*args, **kwargs):
            return real_publish(*args, scanner=scanner, runner=runner, **kwargs)
        with contextlib.ExitStack() as stack:
            stack.enter_context(patch.dict(os.environ, env, clear=True))
            for name in ('event_identity', 'GitHub', 'snapshot'):
                stack.enter_context(patch.object(publication, name))
            authority = stack.enter_context(patch.object(publication, 'Reservations')).return_value
            authority.require_current.return_value = True
            for name, value in [('Registry', registry), ('BuildCache', cache), ('inspect', digest),
                                ('identity', (release, {}, {})), ('try_reuse', None)]:
                stack.enter_context(patch.object(publication, name, return_value=value))
            build = stack.enter_context(patch.object(publication, 'build', return_value=(archive, release)))
            stack.enter_context(patch.object(publication, 'publish', side_effect=publish))
            stack.enter_context(contextlib.redirect_stdout(io.StringIO()))
            self.assertEqual(publication.main(), 1)
        build.assert_called_once()
        scanner.assert_called_once()
        runner.assert_not_called()
        cache.background_export.assert_not_called()
        finish_export.assert_not_called()
        self.assertFalse(Path(env['CD_RECEIPTS_PATH']).exists())

    def test_build_receives_cache_runtime_without_changing_image_arguments(self):
        from staging_cd_build import build
        from staging_cd_build_tests import BuildTests
        from staging_cd_reservations import next_reservation
        source = self.root / 'source'
        source.mkdir()
        BuildTests().source(source)
        cache = Mock()
        cache.build_options.return_value = layer_options(self.env, 'web')
        cache.build_environment.return_value = {'ACTIONS_RUNTIME_TOKEN': 'synthetic-runtime'}
        def write_image(args, **kwargs):
            target = next(arg.split('type=oci,dest=', 1)[1] for arg in args if arg.startswith('type=oci,dest='))
            Path(target).write_bytes(b'synthetic image')
        runner = Mock(side_effect=write_image)
        reservation = next_reservation('web', 'a' * 40, 'b' * 40, 42, 'patch', 1, 1, [], [])
        build(ROOT, reservation, self.root, runner, source=source, cache=cache)
        runner.assert_called_once()
        self.assertEqual(runner.call_args.kwargs['env'], cache.build_environment.return_value)
        self.assertNotIn('synthetic-runtime', ' '.join(runner.call_args.args[0]))
        self.assertNotIn('--secret', runner.call_args.args[0])
        self.assertNotIn('--cache-to', runner.call_args.args[0])
        self.assertIn('--cache-from', runner.call_args.args[0])

    def test_dependency_graph_changes_keep_family_and_runtime_changes_keep_exact_key(self):
        fake = self.root / 'source'
        for relative in ['api/Cargo.lock', 'api/Cargo.toml', 'api/crates/cmaeg/Cargo.toml',
                         'api/rust-toolchain.toml', 'api/crates/cmaeg/Dockerfile', '.github/scripts/deployables.json']:
            target = fake / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes((ROOT / relative).read_bytes())
        base = identity(fake, 'eg')
        day = date(2026, 10, 1)
        # Midnight turns over the exact key, so the registry and anything beyond
        # incremental state refresh, but keeps the family: the next day's first
        # build is seeded with the previous day's incremental state.
        before, after = identity(fake, 'eg', day=day), identity(fake, 'eg', day=date(2026, 10, 2))
        self.assertEqual(before.family, after.family)
        self.assertNotEqual(before.graph, after.graph)
        dockerfile = fake / 'api/crates/cmaeg/Dockerfile'
        original = dockerfile.read_text()
        consumed = original.replace("FROM rust:1.96.1-alpine AS build", "FROM rust:1.96.1-alpine AS build\n"
                                    "ARG OPT_LEVEL\nENV RUSTFLAGS=-Copt-level=${OPT_LEVEL}")
        dockerfile.write_text('ARG OPT_LEVEL=2\n' + consumed)
        first = identity(fake, 'eg', day=day)
        dockerfile.write_text('ARG OPT_LEVEL=3\n' + consumed)
        self.assertNotEqual(first.family, identity(fake, 'eg', day=day).family)
        dockerfile.write_text(original)
        for relative, extra in [('api/Cargo.lock', b'# changed lock\n'),
                                ('api/crates/cmaeg/Cargo.toml', b'\n[build-dependencies.synthetic]\nversion = "1"\n')]:
            target = fake / relative
            before = target.read_bytes()
            target.write_bytes(before + extra)
            changed = identity(fake, 'eg')
            self.assertEqual(base.family, changed.family)
            self.assertNotEqual(base.graph, changed.graph)
            target.write_bytes(before)
        dockerfile = fake / 'api/crates/cmaeg/Dockerfile'
        dockerfile.write_text(dockerfile.read_text() + '\nLABEL example="runtime-only"\n')
        dockerfile.write_text(dockerfile.read_text().replace("RUN --network=none /out/cmaeg --allocator-self-test",
                                                             "RUN --network=none /out/cmaeg --more-self-tests"))
        self.assertEqual(base, identity(fake, 'eg'))
        self.assertEqual(prepare(fake, self.env), {
            "directory": str(self.root / "cma-build-cache/accepted"), "key": base.prefix + "123-1",
            "prefix": base.prefix, "family": base.family})
        for relative, old, new in [('api/rust-toolchain.toml', '1.96.1', '1.97.0'),
                                   ('api/crates/cmaeg/Dockerfile', 'codegen-units=256', 'codegen-units=16'),
                                   ('api/crates/cmaeg/Dockerfile', 'rust:1.96.1-alpine', 'rust:other'),
                                   ('api/Cargo.toml', '[profile.dev.package.adler2]', '[profile.release.package.adler2]')]:
            target = fake / relative
            original = target.read_text()
            target.write_text(original.replace(old, new))
            self.assertNotEqual(base.family, identity(fake, 'eg').family)
            target.write_text(original)
        config = fake / 'api/.cargo/config.toml'
        config.parent.mkdir()
        config.write_text('[build]\ntarget = "synthetic-target"\n')
        self.assertNotEqual(base.family, identity(fake, 'eg').family)

    def test_workflow_persists_cargo_only_after_success_and_uses_pinned_runtime(self):
        workflow = (ROOT / '.github/workflows/staging-release.yml').read_text()
        self.assertGreater(workflow.index('Retain accepted Cargo mounts'),
                           workflow.index('Build once, scan every layer, and publish exact bytes'))
        self.assertIn("github.event_name == 'push' && github.ref == 'refs/heads/develop' && steps.publication.outputs.build-cache-ready == 'true'", workflow)
        self.assertIn("run: python3 -B .github/scripts/staging_cd_cargo_artifact.py", workflow)
        self.assertIn("CD_CARGO_CACHE_FAMILY: ${{ steps.build-cache.outputs.family }}", workflow)
        self.assertIn("CD_CARGO_CACHE_PREFIX: ${{ steps.build-cache.outputs.prefix }}", workflow)
        self.assertIn("name: ${{ steps.publication.outputs.build-cache-artifact }}", workflow)
        self.assertIn("path: ${{ steps.publication.outputs.build-cache-directory }}", workflow)
        self.assertIn("if: steps.cargo-bundle.outcome == 'success'", workflow)
        self.assertIn("name: ${{ steps.publication.outputs.build-cache-index }}", workflow)
        self.assertIn("path: ${{ steps.publication.outputs.build-cache-directory }}/manifest.json", workflow)
        self.assertIn('actions/github-script@ed597411d8f924073f98dfc5c65a23a2325f34cd', workflow)
        self.assertIn('core.setSecret(value)', workflow)
        self.assertNotIn('core.info(value)', workflow)
        self.assertNotIn('type=registry', workflow)
        import yaml
        ci = yaml.safe_load((ROOT / '.github/workflows/ci-images.yml').read_text())
        builds = [step['with'] for step in ci['jobs']['images']['steps']
                  if step.get('uses', '').startswith('docker/build-push-action@')]
        self.assertEqual(len(builds), 2)
        for build in builds:
            for scope in ('cma-am', 'cma-eg', 'cma-web'):
                self.assertIn(scope, build['cache-from'])
            self.assertNotIn('cache-to', build)


class TimingTests(unittest.TestCase):
    def test_runtime_scan_duration_and_cache_counts_stay_sanitized(self):
        timings = Timings(clock=Mock(side_effect=[0, 3, 4, 11]))
        with timings.stage("build"):
            pass
        with timings.stage("scan"):
            timings.detector_cache(dict(hits=4, misses=1, withheld=0, secret="private"))
        for invalid in (None, {}, dict(hits=True, misses=0, withheld=0),
                        dict(hits=0, misses=-1, withheld=0), dict(hits=10**10, misses=0, withheld=0)):
            timings.detector_cache(invalid)
        output = io.StringIO()
        timings.emit(output, scan_only=True)
        self.assertEqual([json.loads(line) for line in output.getvalue().splitlines()], [
            {"event": "publication-detector-cache", "hits": 4, "misses": 1, "withheld": 0},
            {"event": "publication-stage", "stage": "scan", "seconds": 7, "result": "success"},
        ])

    def test_failure_retains_only_fixed_stage_and_duration(self):
        timings = Timings(clock=Mock(side_effect=[10, 12.5]))
        with self.assertRaises(ValueError), timings.stage('scan'):
            raise ValueError('synthetic private response')
        output = io.StringIO()
        timings.emit(output)
        row = json.loads(output.getvalue())
        self.assertEqual(row, {'event': 'publication-stage', 'stage': 'scan', 'seconds': 2.5, 'result': 'failure'})
        self.assertNotIn('private response', output.getvalue())
        with self.assertRaises(ValueError), timings.stage('unknown unsafe name'):
            pass

    def test_progress_categories_never_emit_commands_or_sum_overlap(self):
        records = []
        for n, name, start, end in [(1, 'RUN cargo build --release synthetic-private-argument', 0, 10),
                                    (2, 'RUN cargo build --release', 5, 15),
                                    (3, 'exporting layers', 16, 18), (4, 'RUN private-value', 0, 5),
                                    (5, 'importing cache manifest from private-path', 0, 2),
                                    (6, 'exporting cache to private-target', 17, 20)]:
            records.append({'id': 'sha256:' + str(n) * 64, 'name': name,
                            'started': f'2026-01-01T00:00:{start:02d}Z',
                            'completed': f'2026-01-01T00:00:{end:02d}Z'})
        output = summarize(b'\n'.join(json.dumps(r).encode() for r in records))
        self.assertEqual(output['category_seconds'], {'rust-compile': 15, 'other-build': 5,
                         'archive-export': 2, 'cache-import': 2, 'cache-export': 3,
                         'dependency-compile': 0, 'go-compile': 0, 'media-compile': 0})
        self.assertNotIn('private', json.dumps(output))
        self.assertFalse(summarize(b'not json')['complete'])

    def test_other_native_builds_are_not_counted_as_rust(self):
        rows = [
            {'id': str(index), 'name': name, 'started': '2026-01-01T00:00:00Z',
             'completed': '2026-01-01T00:00:04Z'}
            for index, name in enumerate([
                '[dependencies 2/2] RUN cargo chef cook --release private-value',
                '[secret-scanner-build 5/5] RUN go build private-value',
                '[ffmpeg-build 4/4] RUN ./configure --prefix=private-value; make',
            ])
        ]
        result = summarize(b'\n'.join(json.dumps(row).encode() for row in rows))
        self.assertEqual(result['category_seconds']['rust-compile'], 0)
        for name in ('dependency-compile', 'go-compile', 'media-compile'):
            self.assertEqual(result['category_seconds'][name], 4)
        self.assertNotIn('private-value', json.dumps(result))

    def test_only_successful_cached_dependency_cook_counts_as_reused(self):
        rows = [
            {'id': '1', 'name': '[dependencies 2/2] RUN cargo chef cook --release',
             'cached': True, 'completed': '2026-01-01T00:00:04Z'},
            {'id': '2', 'name': '[dependencies 1/2] COPY recipe.json /recipe.json',
             'cached': True, 'completed': '2026-01-01T00:00:04Z'},
            {'id': '3', 'name': '[dependencies 2/2] RUN cargo chef cook --release',
             'cached': True, 'error': 'failed', 'completed': '2026-01-01T00:00:04Z'},
            {'id': '4', 'name': '[dependencies 2/2] RUN cargo chef cook --release',
             'cached': False, 'completed': '2026-01-01T00:00:04Z'},
        ]
        result = summarize(b'\n'.join(json.dumps(row).encode() for row in rows))
        self.assertEqual(result['cached_dependency_steps'], 1)
        self.assertEqual(result['cached_build_steps'], 2)

    def test_a_failed_build_names_only_the_stage_and_step_where_it_stopped(self):
        rows = [
            {'id': '1', 'name': '[runtime-base 3/12] RUN apt-get install private-value',
             'started': '2026-01-01T00:00:00Z', 'completed': '2026-01-01T00:00:09Z',
             'error': 'exit code: 100 private-error'},
            {'id': '2', 'name': '[codex 2/4] RUN curl private-value',
             'started': '2026-01-01T00:00:00Z', 'completed': '2026-01-01T00:00:10Z',
             'error': 'context canceled'},
            {'id': '3', 'name': '[internal] load metadata for docker.io/library/private:tag',
             'started': '2026-01-01T00:00:00Z', 'completed': '2026-01-01T00:00:01Z'},
            {'id': '4', 'name': '[build 9/9] RUN cargo build private-value',
             'started': '2026-01-01T00:00:00Z', 'completed': '2026-01-01T00:00:02Z'},
        ]
        result = summarize(b'\n'.join(json.dumps(row).encode() for row in rows))
        self.assertEqual(result['failed_steps'], ['runtime-base 3/12', 'codex 2/4'])
        self.assertNotIn('private', json.dumps(result))
        metadata = [{'id': '5', 'name': '[internal] load metadata for ghcr.io/private/image:tag',
                     'started': '2026-01-01T00:00:00Z', 'completed': '2026-01-01T00:00:03Z',
                     'error': 'failed to resolve: private'},
                    {'id': '6', 'name': 'unnamed private vertex', 'error': 'private'}]
        result = summarize(b'\n'.join(json.dumps(row).encode() for row in metadata))
        self.assertEqual(result['failed_steps'], ['other', 'base-image-metadata'])
        self.assertNotIn('private', json.dumps(result))
        self.assertNotIn('failed_steps', summarize(b'\n'.join(json.dumps(row).encode() for row in rows[2:])))
        # BuildKit pads the position to the width of the total in longer stages.
        padded = [{'id': '7', 'name': '[media-self-test  7/11] RUN private-value', 'error': 'private'},
                  {'id': '8', 'name': '[build   9/120] RUN private-value', 'error': 'private',
                   'completed': '2026-01-01T00:00:01Z'}]
        result = summarize(b'\n'.join(json.dumps(row).encode() for row in padded))
        self.assertEqual(result['failed_steps'], ['media-self-test 7/11', 'build 9/120'])
        self.assertNotIn('private', json.dumps(result))

    def test_an_ignored_cache_export_failure_is_cache_evidence_not_a_failed_step(self):
        # BuildKit keeps an ignore-error export failure on its exporter vertex,
        # with the cache service's refusal as go-actions-cache relays it.
        budget = ('error writing layer blob: failed to reserve cache: You have reached your configured budget, '
                  'your cache is now read only to prevent additional charges. private-value')
        step = {'id': '1', 'name': '[build 3/9] RUN cargo build private-value',
                'started': '2026-01-01T00:00:00Z', 'completed': '2026-01-01T00:00:09Z'}
        image = {'id': '2', 'name': 'exporting to oci image format',
                 'started': '2026-01-01T00:00:09Z', 'completed': '2026-01-01T00:00:12Z'}
        export = {'id': '3', 'name': 'exporting to GitHub Actions Cache',
                  'started': '2026-01-01T00:00:12Z', 'completed': '2026-01-01T00:00:22Z'}

        def summary(*rows):
            return summarize(b'\n'.join(json.dumps({'vertexes': [row]}).encode() for row in rows))
        result = summary(step, image, dict(export, error=budget))
        self.assertEqual(result['cache_export_error'], 'read-only')
        self.assertNotIn('failed_steps', result)
        self.assertEqual(result['category_seconds']['cache-export'], 10)
        self.assertEqual(result['category_seconds']['archive-export'], 3)
        for withheld in ('private', 'budget', 'reserve'):
            self.assertNotIn(withheld, json.dumps(result))
        # Another export failure is not explained, and a real step failure still reports.
        result = summary(dict(step, error='exit code: 101'), dict(export, error='maximum timeout reached private'))
        self.assertEqual((result['cache_export_error'], result['failed_steps']), ('failed', ['build 3/9']))
        self.assertNotIn('private', json.dumps(result))
        self.assertNotIn('cache_export_error', summary(step, image, export))

    def test_a_failed_test_stage_keeps_its_names_beside_a_failed_cache_export(self):
        # The export vertex is cache evidence, never a second failed step or a
        # source of test names; the failed test stage still names its tests.
        import staging_cd_test_failures_tests as failures
        raw = failures.progress(
            (1, failures.QUALIFICATION, 'process did not complete successfully: private', {1: failures.SPEC}),
            (2, 'exporting to GitHub Actions Cache',
             'failed to reserve cache: You have reached your configured budget, private', {}))
        result = summarize(raw, dockerfile=failures.RUNTIME)
        self.assertEqual(result['failed_steps'], [failures.STEP])
        self.assertEqual(result['failed_tests'], failures.SPEC_NAMES)
        self.assertEqual(result['cache_export_error'], 'read-only')
        self.assertNotIn('private', json.dumps(result))


if __name__ == '__main__':
    unittest.main()
