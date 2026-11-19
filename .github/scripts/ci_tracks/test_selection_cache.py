"""Cache misses and invalid identities must run selection again."""

import hashlib
import json
from pathlib import Path
import tempfile
import unittest

from selection_cache import SelectionCache, canonical, fingerprint


def result():
    return {'base': 'a' * 40, 'head': 'b' * 40, 'repo': True, 'am': False, 'eg': False, 'web': True,
            'reasons': ['web-source'], 'affected': [], 'am_packages': [], 'eg_packages': [],
            'test_tracks': {'repo': True, 'am': False, 'eg': False, 'web': True},
            'deployables': ['cma-web'], 'changed_path_count': 1}


class SelectionCacheTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.cache = SelectionCache(self.root / 'cache', 'c' * 64)
        self.value = result()

    def load(self):
        return self.cache.load(self.value['base'], self.value['head'])

    def test_round_trip_exact_identity_and_bounded_retention(self):
        self.assertIsNone(self.load())
        self.cache.save(self.value)
        self.assertEqual(self.load(), self.value)
        self.assertIsNone(self.cache.load('d' * 40, self.value['head']))
        other = dict(self.value, head='d' * 40)
        self.cache.save(other)
        self.cache.retain([self.value['head']])
        self.assertFalse(self.cache.path(other['head']).exists())

    def test_policy_change_and_corrupt_checksum_miss(self):
        self.cache.save(self.value)
        changed = SelectionCache(self.cache.directory, 'd' * 64)
        self.assertIsNone(changed.load(self.value['base'], self.value['head']))
        path = self.cache.path(self.value['head'])
        record = json.loads(path.read_bytes())
        record['result']['deployables'] = []
        path.write_bytes(canonical(record))
        self.assertIsNone(self.load())

    def test_invalid_result_schema_even_with_checksum_misses(self):
        for updates in ({'head': 'd' * 40}, {'deployables': ['other']}, {'repo': False},
                        {'deployables': ['cma-runtime-desktop']}, {'deployables': ['cma-image-manager']},
                        {'web': 'true'}, {'changed_path_count': -1}, {'extra': 'ignored'},
                        {'deployables': ['cma-web', 'cma-web']}, {'reasons': ['input-analysis-uncertain']}):
            self.cache.save(self.value)
            path = self.cache.path(self.value['head'])
            record = json.loads(path.read_bytes())
            record['result'].update(updates)
            record['checksum'] = hashlib.sha256(canonical(record['result'])).hexdigest()
            path.write_bytes(canonical(record))
            self.assertIsNone(self.load())

    def test_truncated_oversized_or_symlink_record_misses(self):
        path = self.cache.path(self.value['head'])
        for data in (b'{', b'x' * 65537):
            path.write_bytes(data)
            self.assertIsNone(self.load())
        path.unlink()
        target = self.root / 'target'
        target.write_text('unchanged')
        path.symlink_to(target)
        self.assertIsNone(self.load())
        self.cache.save(self.value)
        self.assertEqual(target.read_text(), 'unchanged')
        self.assertFalse(path.is_symlink())

    def test_unexpected_entries_cannot_be_reexported(self):
        (self.cache.directory / 'unexpected').write_text('fixture')
        with self.assertRaises(ValueError):
            self.cache.retain([])
        with self.assertRaises(ValueError):
            self.cache.path('../outside')
        with self.assertRaises(ValueError):
            self.cache.save({'head': 'b' * 40})

    def test_each_policy_input_change_invalidates_the_cache_namespace(self):
        paths = ['.github/scripts/ci_tracks/selection.py', '.github/scripts/ci_tracks/requirements.txt',
                 '.github/scripts/ci_tracks/publication_inputs.py',
                 'api/rust-toolchain.toml', '.github/scripts/deployables.json',
                 '.github/workflows/staging-cd.yml', '.github/workflows/ci.yml',
                 '.github/scripts/staging_cd_queue.py',
                 '.github/scripts/staging_cd_selection_cache.py']
        for name in paths:
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text('initial')
        before = fingerprint(self.root)
        for name in paths:
            path = self.root / name
            path.write_text('changed')
            self.assertNotEqual(fingerprint(self.root), before)
            path.write_text('initial')
        self.assertEqual(fingerprint(self.root), before)
        (self.root / paths[0]).unlink()
        (self.root / paths[0]).symlink_to(self.root / paths[1])
        with self.assertRaises(ValueError):
            fingerprint(self.root)



    def test_fallback_context_change_recomputes_comparison(self):
        self.cache.save(self.value, 'a' * 64)
        self.assertIsNone(self.load())
        self.assertEqual(self.cache.load(self.value['base'], self.value['head'], 'a' * 64), self.value)
        self.assertIsNone(self.cache.load(self.value['base'], self.value['head'], 'b' * 64))

    def test_input_map_identity_integrity_and_retention(self):
        from selection_cache import IMAGES
        inputs = {name: {'api/source.rs'} for name in IMAGES}
        head, context = self.value['head'], 'source-manifests'
        self.cache.save_inputs(head, context, inputs)
        self.assertEqual(self.cache.load_inputs(head, context), inputs)
        self.assertIsNone(self.cache.load_inputs(head, 'a' * 64))
        self.assertIsNone(SelectionCache(self.cache.directory, 'f' * 64).load_inputs(head, context))
        path = self.cache.directory / ('inputs-' + head + '.json')
        original = path.read_bytes()
        for updates in ({'revision': 'e' * 40}, {'inputs': {}}, {'inputs': {name: ['../outside'] for name in IMAGES}},
                        {'inputs': {name: ['api/source.rs', 'api/source.rs'] for name in IMAGES}}):
            record = json.loads(original)
            record.update(updates)
            record['checksum'] = hashlib.sha256(canonical(record['inputs'])).hexdigest()
            path.write_bytes(canonical(record))
            self.assertIsNone(self.cache.load_inputs(head, context))
        path.write_bytes(original.replace(b'api/source.rs', b'api/other.rs'))
        self.assertIsNone(self.cache.load_inputs(head, context))
        path.write_bytes(b'x' * (8 * 1024 * 1024 + 1))
        self.assertIsNone(self.cache.load_inputs(head, context))
        path.unlink()
        target = self.root / 'target'
        target.write_bytes(original)
        path.symlink_to(target)
        self.assertIsNone(self.cache.load_inputs(head, context))
        self.cache.save_inputs(head, context, inputs)
        self.assertFalse(path.is_symlink())
        self.cache.retain([head])
        self.assertTrue(path.exists())
        self.cache.retain(['d' * 40])
        self.assertFalse(path.exists())


if __name__ == '__main__':
    unittest.main()
