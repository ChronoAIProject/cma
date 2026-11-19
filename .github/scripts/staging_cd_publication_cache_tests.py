"""Protected cache writers cannot bypass fresh image publication gates."""
import io
import contextlib
import json
import os
import subprocess
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
                    "RUNNER_TEMP": str(self.root)}

    def cache(self):
        return BuildCache(ROOT, self.env, ROOT, "eg")

    def populate(self, directory):
        directory.mkdir()
        for name in ('registry', 'target'):
            with tarfile.open(directory / (name + '.tar.gz'), 'w:gz') as archive:
                member = tarfile.TarInfo('synthetic-data')
                member.size = 4
                archive.addfile(member, io.BytesIO(b'data'))

    def test_layer_cache_exports_only_on_protected_develop_pushes(self):
        for event, ref, writer in [('push', 'refs/heads/develop', True),
                                   ('push', 'refs/heads/main', False),
                                   ('push', 'refs/heads/topic', False),
                                   ('pull_request', 'refs/pull/12/merge', False),
                                   ('workflow_dispatch', 'refs/heads/develop', False),
                                   ('pull_request_target', 'refs/heads/develop', False)]:
            args = layer_options(dict(self.env, GITHUB_EVENT_NAME=event, GITHUB_REF=ref), 'eg')
            self.assertIn('--cache-from', args)
            self.assertEqual('--cache-to' in args, writer)
            self.assertEqual(any('mode=max' in arg for arg in args), writer)
            self.assertTrue(all('type=gha,version=2,scope=cma-eg,' in arg for arg in args if not arg.startswith('--')))
        self.assertEqual(len(set(SCOPES.values())), 5)
        for component, scope in SCOPES.items():
            self.assertIn('scope=' + scope + ',', layer_options(self.env, component)[1])
        for env, component in [(dict(self.env, GITHUB_REPOSITORY='fork/repo'), 'eg'), (self.env, 'unknown')]:
            with self.assertRaises(Refusal):
                layer_options(env, component)

    def test_runtime_builds_read_shared_stages_and_export_only_their_own_scope(self):
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
                    self.assertEqual(exports, [imports[0] + ",mode=max,ignore-error=true"] if event == "push" else [])

    def test_only_develop_producers_can_prepare_or_transfer_cargo_caches(self):
        for key, value in (("GITHUB_EVENT_NAME", "pull_request"), ("GITHUB_EVENT_NAME", "workflow_dispatch"),
                           ("GITHUB_REF", "refs/pull/12/merge"), ("GITHUB_REPOSITORY", "fork/repo"),
                           ("CD_COMPONENT", "unrecognized"), ("GITHUB_RUN_ID", "bad")):
            with self.subTest(key=key), self.assertRaises(Refusal):
                context({**self.env, key: value})
        self.assertIn('cma-cd-cargo-v2-eg-', prepare(ROOT, self.env)['prefix'])

    def test_validated_mounts_restore_and_corrupt_content_builds_cold(self):
        cache = self.cache()
        self.assertFalse(cache.restore())
        self.populate(cache.accepted)
        tool = bridge()
        tool.seal(cache.accepted, cache.compatibility)
        with patch.object(tool, 'transfer') as transfer, patch('staging_cd_build_cache.bridge', return_value=tool):
            self.assertTrue(cache.restore())
            transfer.assert_called_once_with('inject', cache.accepted)
            self.assertFalse(cache.export_mounts())
            (cache.accepted / 'target.tar.gz').write_bytes(b'changed')
            self.assertFalse(self.cache().restore())
            self.assertEqual(transfer.call_count, 1)

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
        with patch.object(tool, 'transfer', side_effect=lambda operation, directory: self.populate(directory)), \
                patch('staging_cd_build_cache.bridge', return_value=tool):
            self.assertTrue(cache.export_mounts())
            tool.validate_bundle(cache.accepted, cache.compatibility)
            self.assertFalse(cache.pending.exists())

    def test_cache_credentials_are_only_passed_to_the_buildx_client(self):
        self.assertEqual(self.cache().build_options(), [])
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
        cache = Mock(restored=True)
        cache.restore.return_value = True
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
        cache.export_mounts.assert_not_called()
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
        self.assertIn('--cache-to', runner.call_args.args[0])

    def test_toolchain_lock_recipe_and_config_changes_invalidate_cargo_identity(self):
        fake = self.root / 'source'
        for relative in ['api/Cargo.lock', 'api/Cargo.toml', 'api/rust-toolchain.toml',
                         '.dockerignore', 'api/crates/cmaeg/Dockerfile', '.github/scripts/deployables.json']:
            target = fake / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes((ROOT / relative).read_bytes())
        base = identity(fake, 'eg')
        for relative in ['api/Cargo.lock', 'api/Cargo.toml', 'api/rust-toolchain.toml',
                         'api/crates/cmaeg/Dockerfile', '.dockerignore']:
            target = fake / relative
            before = target.read_bytes()
            target.write_bytes(before + b'\n')
            self.assertNotEqual(base, identity(fake, 'eg'))
            target.write_bytes(before)
        target = fake / 'api/.cargo/config.toml'
        target.parent.mkdir()
        target.write_text('[build]\nrustflags = []\n')
        self.assertNotEqual(base, identity(fake, 'eg'))

    def test_workflow_persists_cargo_only_after_success_and_uses_pinned_runtime(self):
        workflow = (ROOT / '.github/workflows/staging-release.yml').read_text()
        self.assertGreater(workflow.index('Retain accepted Cargo mounts'),
                           workflow.index('Build once, scan every layer, and publish exact bytes'))
        self.assertIn("github.event_name == 'push' && github.ref == 'refs/heads/develop' && steps.publication.outputs.build-cache-ready == 'true'", workflow)
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
            if 'cache-to' in build:
                self.assertIn("github.event_name == 'push' && github.ref == 'refs/heads/develop'", build['cache-to'])


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


if __name__ == '__main__':
    unittest.main()
