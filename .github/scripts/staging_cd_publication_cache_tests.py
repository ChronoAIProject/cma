"""Cache acceptance cannot export discarded secrets or bypass release checks."""
import hashlib
import io
import json
import os
import tarfile
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock

from image_secret_gate import ScanRefusal
from staging_cd_build_cache import BuildCache, cache_archive, files, identity
from staging_cd_build_progress import summarize
from staging_cd_common import Refusal
from staging_cd_scan_cache import context, options, prepare
from staging_cd_timing import Timings


ROOT = Path(__file__).resolve().parents[2]


class CacheTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.env = {"GITHUB_REPOSITORY": "ChronoAIProject/cma", "GITHUB_EVENT_NAME": "push",
                    "GITHUB_REF": "refs/heads/develop", "GITHUB_SHA": "a" * 40,
                    "GITHUB_RUN_ID": "123", "GITHUB_RUN_ATTEMPT": "1", "CD_COMPONENT": "web",
                    "RUNNER_TEMP": str(self.root)}

    def cache(self, scanner=None):
        return BuildCache(ROOT, self.env, ROOT, "web", scanner=scanner or Mock())

    def populate(self, cache):
        args = cache.build_options()
        self.assertTrue(any(s.startswith("type=local,mode=max,") for s in args))
        self.assertFalse(any("type=gha" in s or "type=registry" in s for s in args))
        data = cache.pending / "buildkit/blobs/sha256"
        data.mkdir(parents=True)
        (data / hashlib.sha256(b"synthetic").hexdigest()).write_bytes(b"synthetic")
        (cache.pending / "buildkit/index.json").write_text('{}')

    def test_only_develop_producers_can_enable_or_prepare_caches(self):
        for key, value in (("GITHUB_EVENT_NAME", "pull_request"), ("GITHUB_EVENT_NAME", "workflow_dispatch"),
                           ("GITHUB_REF", "refs/pull/12/merge"), ("GITHUB_REPOSITORY", "fork/repo"),
                           ("CD_COMPONENT", "unrecognized"), ("GITHUB_RUN_ID", "bad")):
            with self.subTest(key=key), self.assertRaises(Refusal):
                context({**self.env, key: value})
        values = prepare(ROOT, self.env)
        self.assertIn('cma-cd-detector-v1-web-', values['prefix'])
        self.assertEqual(options(self.env), {})
        self.assertEqual(options({**self.env, "CD_DETECTOR_CACHE": "enabled"})['detector_cache'], None)

    def test_qualified_export_restores_exact_closure_and_changed_bytes_miss(self):
        scanner = Mock()
        cache = self.cache(scanner)
        self.assertFalse(cache.restore())
        self.populate(cache)
        self.assertTrue(cache.qualify())
        scanner.assert_called_once()
        self.assertFalse(cache.pending.exists())
        warm = self.cache()
        self.assertTrue(warm.restore())
        args = warm.build_options()
        self.assertIn("--cache-from", args)
        (warm.accepted / 'data/buildkit/index.json').write_text('{"changed":true}')
        self.assertFalse(self.cache().restore())

    def test_discarded_stage_or_cargo_scan_failure_never_becomes_exportable(self):
        cache = self.cache(Mock(side_effect=ScanRefusal("synthetic blocked content")))
        self.populate(cache)
        self.assertFalse(cache.qualify())
        self.assertFalse(cache.accepted.exists())
        self.assertFalse(cache.pending.exists())
        self.assertFalse((cache.root / 'qualification.tar').exists())

    def test_cache_archive_contains_every_exported_file_not_only_final_image(self):
        pending = self.root / 'pending'
        pending.mkdir()
        (pending / 'discarded-stage.bin').write_bytes(b'synthetic discarded content')
        (pending / 'cargo').mkdir()
        (pending / 'cargo/target.tar.gz').write_bytes(b'synthetic dependency content')
        target = self.root / 'cache.tar'
        inventory = cache_archive(pending, target)
        self.assertEqual(set(inventory), {'discarded-stage.bin', 'cargo/target.tar.gz'})
        with tarfile.open(target) as archive:
            with tarfile.open(fileobj=archive.extractfile('layer.tar')) as layer:
                self.assertEqual(set(layer.getnames()), set(inventory))

    def test_symlink_and_special_cache_paths_refuse(self):
        data = self.root / 'data'
        data.mkdir()
        (self.root / 'outside').write_text('synthetic')
        (data / 'link').symlink_to(self.root / 'outside')
        with self.assertRaises(Refusal):
            files(data)
        (data / 'link').unlink()
        os.mkfifo(data / 'fifo')
        with self.assertRaises(Refusal):
            files(data)

    def test_toolchain_lock_recipe_or_scanner_identity_changes_invalidate(self):
        fake = self.root / 'source'
        for relative in ['api/Cargo.lock', 'api/Cargo.toml', 'api/rust-toolchain.toml',
                         '.dockerignore', 'web/Dockerfile', '.github/scripts/deployables.json']:
            target = fake / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes((ROOT / relative).read_bytes())
        base = identity(fake, 'web')
        for relative in ['api/Cargo.lock', 'api/rust-toolchain.toml', 'web/Dockerfile', '.dockerignore']:
            target = fake / relative
            before = target.read_bytes()
            target.write_bytes(before + b'\n')
            self.assertNotEqual(base, identity(fake, 'web'))
            target.write_bytes(before)

    def test_scan_export_cannot_change_after_qualification(self):
        def scanner(_archive, _report):
            (cache.pending / 'buildkit/index.json').write_text('altered')
        cache = self.cache(scanner)
        self.populate(cache)
        self.assertFalse(cache.qualify())
        self.assertFalse(cache.accepted.exists())

    def test_workflow_export_is_after_publication_and_never_direct_remote_build(self):
        workflow = (ROOT / '.github/workflows/staging-release.yml').read_text()
        save = workflow.index('Retain only the qualified build cache closure')
        build = workflow.index('Build once, scan every layer, and publish exact bytes')
        self.assertGreater(save, build)
        self.assertIn("steps.publication.outputs.build-cache-ready == 'true'", workflow)
        self.assertIn("github.event_name == 'push' && github.ref == 'refs/heads/develop'", workflow)
        self.assertNotIn('type=registry', workflow)


class TimingTests(unittest.TestCase):
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
                                    (3, 'exporting layers', 16, 18), (4, 'RUN private-value', 0, 5)]:
            records.append({'id': 'sha256:' + str(n) * 64, 'name': name,
                            'started': f'2026-01-01T00:00:{start:02d}Z',
                            'completed': f'2026-01-01T00:00:{end:02d}Z'})
        output = summarize(b'\n'.join(json.dumps(r).encode() for r in records))
        self.assertEqual(output['category_seconds'], {'rust-compile': 15, 'other-build': 5, 'export': 2})
        self.assertNotIn('private', json.dumps(output))
        self.assertFalse(summarize(b'not json')['complete'])


if __name__ == '__main__':
    unittest.main()
