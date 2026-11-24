"""A metadata inventory must not silently turn absent collections into passes."""
from copy import deepcopy
import hashlib
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from migration_catalog import STAGES
from migration_catalog_coverage import audit, main, read_bound
from test_migration_catalog import snapshot


class CoverageTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.directory = Path(directory.name)
        self.inventory = {'schema': 'cma-reader-writer-source-inventory-v1',
            'old': {'source': 'a' * 40, 'tree': 'b' * 40},
            'new': {'source': 'c' * 40, 'tree': 'd' * 40},
            'collections': [{'name': name, 'old': {}, 'new': {}}
                            for name in ('sandbox_usage_events', 'unexercised')]}
        self.evidence = {'schema': 'cma-migration-kind-evidence-v1', 'outcome': 'UNVERIFIED',
                         'catalogs': []}
        for role, character in [('old', '1'), ('new', '2')]:
            self.evidence[role] = {'source': self.inventory[role]['source'],
                'image': 'registry.local/api@sha256:' + character * 64,
                'config': 'sha256:' + character * 64}
        for stage in STAGES:
            role = 'old' if stage in (STAGES[0], STAGES[3]) else 'new'
            row = {'stage': stage, **self.evidence[role], 'collections': 1,
                'path': 'catalog-' + stage + '.private.json',
                'serving_pod': {'uid': role, 'image_id': self.evidence[role]['config']}}
            self.evidence['catalogs'].append(row)
            self.save_snapshot(stage, snapshot())

    def save_snapshot(self, stage, value):
        row = next(row for row in self.evidence['catalogs'] if row['stage'] == stage)
        path = self.directory / row['path']
        path.write_text(json.dumps(value))
        row['sha256'] = hashlib.sha256(path.read_bytes()).hexdigest()
        row['collections'] = len(value['collections'])

    def run_audit(self):
        return audit(self.inventory, self.evidence, self.directory)

    def test_complete_snapshots_leave_absent_collections_and_document_cells_unverified(self):
        result = self.run_audit()
        self.assertEqual(result['summary']['observed_source_collections'], 1)
        self.assertEqual(result['summary']['unobserved_source_collections'], ['unexercised'])
        self.assertEqual(result['summary']['missing_stages'], [])
        self.assertEqual(result['application_compatibility'], 'NOT_ASSESSED')
        self.assertEqual(result['campaign_outcome'], 'UNVERIFIED')
        self.assertEqual({cell['status'] for cell in result['collections'][1]['stages'].values()},
                         {'ABSENT_IN_SNAPSHOT'})
        self.assertEqual({row['outcome'] for row in result['comparisons']}, {'PRESERVED'})

    def test_failed_rollback_is_missing_evidence_never_replaced_by_another_stage(self):
        self.evidence['catalogs'].pop()
        self.evidence['outcome'] = 'FAIL'
        result = self.run_audit()
        self.assertEqual(result['summary']['missing_stages'], [STAGES[3]])
        self.assertEqual(result['campaign_outcome'], 'FAIL')
        self.assertEqual(result['comparisons'][-1]['outcome'], 'NOT_CAPTURED')
        self.assertTrue(all(row['stages'][STAGES[3]] == {'status': 'NOT_CAPTURED'}
                            for row in result['collections']))

    def test_no_snapshots_is_explicitly_missing_not_empty_catalog_success(self):
        self.evidence['catalogs'] = []
        result = self.run_audit()
        self.assertEqual(result['summary']['missing_stages'], list(STAGES))
        self.assertEqual(result['summary']['observed_source_collections'], 0)

    def test_unexpected_namespace_is_retained_for_inventory_review(self):
        value = snapshot()
        value['collections'][0]['name'] = 'new_dynamic_collection'
        self.save_snapshot(STAGES[2], value)
        result = self.run_audit()
        self.assertEqual(result['summary']['unexpected_collections'], ['new_dynamic_collection'])
        self.assertEqual(result['comparisons'][1]['outcome'], 'CHANGED')
        cell = next(row for row in result['collections'] if row['collection'] == 'new_dynamic_collection')
        self.assertFalse(cell['in_source_inventory'])

    def test_index_addition_and_rollback_removal_remain_visible(self):
        value = snapshot()
        value['collections'][0]['indexes'].append({'name': 'new_index', 'key': [['revision', 1]]})
        self.save_snapshot(STAGES[1], value)
        self.save_snapshot(STAGES[2], value)
        result = self.run_audit()
        self.assertEqual(result['comparisons'][0]['additions_requiring_review'][0]['index'], 'new_index')
        self.assertEqual(result['comparisons'][-1]['outcome'], 'CHANGED')
        cell = result['collections'][0]['stages']
        self.assertNotEqual(cell[STAGES[0]]['definition_sha256'], cell[STAGES[1]]['definition_sha256'])

    def test_wrong_source_manifest_config_or_count_cannot_bind_a_snapshot(self):
        mutations = [lambda d: d['old'].update(source='e' * 40),
            lambda d: d['old'].update(image='registry.local/api:latest'),
            lambda d: d['new'].update(config='not-a-config'),
            lambda d: d['catalogs'][0].update(source='e' * 40),
            lambda d: d['catalogs'][0].update(image=d['new']['image']),
            lambda d: d['catalogs'][0]['serving_pod'].update(image_id=d['new']['config']),
            lambda d: d['catalogs'][0]['serving_pod'].update(uid=''),
            lambda d: d['catalogs'][0].update(collections=True),
            lambda d: d['catalogs'][0].update(collections=2)]
        original = deepcopy(self.evidence)
        for mutate in mutations:
            self.evidence = deepcopy(original)
            mutate(self.evidence)
            with self.subTest(mutation=mutate), self.assertRaises(ValueError):
                self.run_audit()

    def test_malformed_inventory_or_campaign_is_rejected(self):
        original = deepcopy(self.inventory)
        mutations = [lambda d: d.update(schema='wrong'), lambda d: d.update(collections=[]),
            lambda d: d['old'].update(tree='short'),
            lambda d: d['collections'].append(d['collections'][0]),
            lambda d: d['collections'][0].update(name='illegal/name'),
            lambda d: d['collections'][0].update(old=None, new=None)]
        for mutate in mutations:
            self.inventory = deepcopy(original)
            mutate(self.inventory)
            with self.subTest(mutation=mutate), self.assertRaises(ValueError):
                self.run_audit()
        self.inventory = original
        original_evidence = deepcopy(self.evidence)
        for mutate in (lambda d: d.update(schema='wrong'), lambda d: d.update(catalogs={}),
                       lambda d: d['catalogs'].append(d['catalogs'][0]),
                       lambda d: d['catalogs'][0].update(stage='other')):
            self.evidence = deepcopy(original_evidence)
            mutate(self.evidence)
            with self.subTest(mutation=mutate), self.assertRaises(ValueError):
                self.run_audit()

    def test_tampered_snapshot_hash_path_and_symlink_are_rejected(self):
        row = self.evidence['catalogs'][0]
        path = self.directory / row['path']
        original = path.read_bytes()
        path.write_bytes(original + b' ')
        with self.assertRaisesRegex(ValueError, 'hash differs'):
            self.run_audit()
        path.write_bytes(original)
        for filename in ('../other.json', str(path)):
            with patch.dict(row, {'path': filename}), self.assertRaisesRegex(ValueError, 'path differs'):
                self.run_audit()
        target = self.directory / 'target.json'
        target.write_bytes(original)
        path.unlink()
        path.symlink_to(target)
        with self.assertRaisesRegex(ValueError, 'path escapes'):
            self.run_audit()

    def test_cli_binds_inputs_preserves_previous_audit_and_rejects_bad_hash(self):
        args = []
        for key, data in [('inventory', self.inventory), ('evidence', self.evidence)]:
            path = self.directory / (key + '.json')
            path.write_text(json.dumps(data))
            args += ['--' + key, str(path), '--' + key + '-sha256',
                     hashlib.sha256(path.read_bytes()).hexdigest()]
        output = self.directory / 'audit.json'
        args += ['--output', str(output)]
        with patch('sys.stdout', new_callable=io.StringIO) as stdout:
            main(args)
        result = json.loads(output.read_text())
        self.assertEqual(result['input_bindings']['inventory_sha256'], args[3])
        self.assertEqual(json.loads(stdout.getvalue())['sha256'], hashlib.sha256(output.read_bytes()).hexdigest())
        with self.assertRaises(FileExistsError):
            main(args)
        for digest in ('invalid', '0' * 64):
            with self.assertRaises(ValueError):
                read_bound(output, digest)


if __name__ == '__main__':
    unittest.main()
