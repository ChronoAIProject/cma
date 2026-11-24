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

    def policy_fixture(self):
        files = ['.github/scripts/ci_tracks/selection.py', '.github/scripts/ci_tracks/requirements.txt',
                 '.github/scripts/ci_tracks/publication_inputs.py', 'api/rust-toolchain.toml',
                 '.github/scripts/deployables.json', '.github/scripts/staging_cd_admission.py',
                 '.github/scripts/staging_cd_comparisons.py', '.github/scripts/staging_cd_selection_cache.py']
        for name in files:
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text('initial')
        workflows = self.root / '.github/workflows'
        workflows.mkdir()
        for name, job in [('staging-cd', 'plan'), ('ci', 'changes')]:
            (workflows / (name + '.yml')).write_text(
                'on: push\npermissions: {contents: read}\njobs:\n  ' + job + ':\n'
                '    runs-on: ubuntu-24.04\n    steps: [{run: select}]\n'
                '  allocate:\n    concurrency: {group: reservations}\n    steps: [{run: reserve}]\n')
        return files

    def test_each_policy_input_change_invalidates_the_cache_namespace(self):
        files = self.policy_fixture()
        before = fingerprint(self.root)
        SelectionCache(self.cache.directory, before).save(self.value)
        for name in files:
            path = self.root / name
            path.write_text('changed')
            after = fingerprint(self.root)
            self.assertNotEqual(after, before, name)
            self.assertIsNone(SelectionCache(self.cache.directory, after).load(self.value['base'], self.value['head']))
            path.write_text('initial')
        self.assertEqual(fingerprint(self.root), before)
        (self.root / files[0]).unlink()
        (self.root / files[0]).symlink_to(self.root / files[1])
        with self.assertRaises(ValueError):
            fingerprint(self.root)

    def test_queue_assembly_and_scheduling_preserve_comparisons_but_not_producer_changes(self):
        self.policy_fixture()
        path = self.root / '.github/workflows/staging-cd.yml'
        original = path.read_text()
        before = fingerprint(self.root)
        SelectionCache(self.cache.directory, before).save(self.value)
        (self.root / '.github/scripts/staging_cd_queue.py').write_text('# assemble the current transition')
        path.write_text('concurrency: {group: earlier, queue: max}\n' +
                        original.replace('    concurrency: {group: reservations}\n', ''))
        after = fingerprint(self.root)
        self.assertEqual(before, after)
        self.assertEqual(SelectionCache(self.cache.directory, after).load(self.value['base'], self.value['head']),
                         self.value)
        for filename in ('staging-cd.yml', 'ci.yml'):
            path = self.root / '.github/workflows' / filename
            baseline = path.read_text()
            for old, new in [('on: push', 'on: pull_request'), ('contents: read', 'contents: write'),
                             ('run: select', 'run: different'), ('runs-on: ubuntu-24.04', 'runs-on: other')]:
                with self.subTest(filename=filename, field=old):
                    path.write_text(baseline.replace(old, new))
                    updated = fingerprint(self.root)
                    self.assertNotEqual(updated, before)
                    self.assertIsNone(SelectionCache(self.cache.directory, updated).load(
                        self.value['base'], self.value['head']))
            path.write_text(baseline)

    def test_ci_event_and_label_changes_preserve_staging_comparisons(self):
        self.policy_fixture()
        scripts = self.root / '.github/scripts/ci_tracks'
        files = [scripts / name for name in ('ci_events.py', 'version_label.py')]
        for path in files:
            path.write_text('initial')
        before = fingerprint(self.root)
        SelectionCache(self.cache.directory, before).save(self.value)
        for path in files:
            with self.subTest(script=path.name):
                path.write_text('changed')
                after = fingerprint(self.root)
                self.assertEqual(after, before)
                self.assertEqual(SelectionCache(self.cache.directory, after).load(
                    self.value['base'], self.value['head']), self.value)

    def test_producer_dependency_changes_invalidate_existing_facts(self):
        self.policy_fixture()
        path = self.root / '.github/workflows/staging-cd.yml'
        workflow = path.read_text().replace('  plan:\n', '  plan:\n    needs: allocate\n')
        path.write_text(workflow)
        before = fingerprint(self.root)
        path.write_text(workflow.replace('run: reserve', 'run: changed'))
        self.assertNotEqual(fingerprint(self.root), before)

    def test_absent_or_ambiguous_producer_fails_before_cache_access(self):
        self.policy_fixture()
        workflow = self.root / '.github/workflows/staging-cd.yml'
        for contents in ('jobs: []', 'jobs: {not-plan: {}}', 'jobs: {plan: {}, plan: {}}',
                         'jobs: {plan: {<<: {steps: []}}}'):
            with self.subTest(contents=contents), self.assertRaises(ValueError):
                workflow.write_text(contents)
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
