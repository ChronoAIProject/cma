import copy
import json
from pathlib import Path
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import patch

import migration_catalog as catalog
from migration_kind_manifests import manifests
import migration_kind_runner as runner


def snapshot():
    return {'schema': catalog.SCHEMA, 'database': 'matrix', 'collections': [{
        'name': 'sandbox_usage_events', 'type': 'collection', 'uuid': 'uuid-1',
        'options': {'validator': {'document_schema_version': {'$eq': 1}}},
        'indexes': [{'v': 2, 'name': '_id_', 'key': [['_id', 1]]}]}]}


def interval():
    return {'v': 2, 'name': 'sandbox_usage_events_interval',
            'key': [['sandbox_id', 1], ['ended_at', 1], ['started_at', 1], ['revision', 1]]}


class CatalogTests(unittest.TestCase):
    def test_upgrade_addition_must_survive_rollback(self):
        old, new = snapshot(), snapshot()
        new['collections'][0]['indexes'].append(interval())
        upgrade = catalog.compare(old, new)
        self.assertEqual(upgrade['outcome'], 'PRESERVED')
        self.assertEqual(upgrade['additions_requiring_review'], [{
            'collection': 'sandbox_usage_events', 'index': interval()['name'],
            'change': 'index_added'}])
        self.assertEqual(catalog.compare(new, copy.deepcopy(new))['outcome'], 'PRESERVED')
        self.assertEqual(catalog.compare(new, old)['outcome'], 'CHANGED')

    def test_compound_order_unique_ttl_partial_and_collation_changes_are_detected(self):
        old = snapshot()
        old['collections'][0]['indexes'].append(interval())
        changes = [lambda index: index['key'].reverse(),
                   lambda index: index.update(unique=True),
                   lambda index: index.update(expireAfterSeconds=0),
                   lambda index: index.update(partialFilterExpression={'active': True}),
                   lambda index: index.update(collation={'locale': 'en'}),
                   lambda index: index.update(hidden=True)]
        for change in changes:
            new = copy.deepcopy(old)
            change(new['collections'][0]['indexes'][1])
            with self.subTest(change=change):
                self.assertEqual(catalog.compare(old, new)['outcome'], 'CHANGED')

    def test_drop_recreate_validator_and_collection_type_changes_are_detected(self):
        for field, value in [('uuid', 'uuid-2'), ('options', {}), ('type', 'timeseries')]:
            new = snapshot()
            new['collections'][0][field] = value
            self.assertEqual(catalog.compare(snapshot(), new)['changes'][0]['change'],
                             field + '_changed')
        new = snapshot()
        new['collections'][0]['name'] = 'replacement'
        result = catalog.compare(snapshot(), new)
        self.assertEqual(result['changes'][0]['change'], 'collection_removed')
        self.assertEqual(result['additions_requiring_review'][0]['change'], 'collection_added')

    def test_malformed_and_incomplete_snapshots_are_rejected(self):
        cases = [None, {}, {**snapshot(), 'database': 'admin'},
                 {**snapshot(), 'collections': []}]
        for field, value in [('uuid', None), ('indexes', []), ('options', []),
                             ('indexes', [{'name': '_id_', 'key': {'_id': 1}}]),
                             ('indexes', [{'name': '_id_', 'key': [['_id', 1]], 'spec': {}}])]:
            data = snapshot()
            data['collections'][0][field] = value
            cases.append(data)
        duplicate = snapshot()
        duplicate['collections'].append(copy.deepcopy(duplicate['collections'][0]))
        cases.append(duplicate)
        for data in cases:
            with self.subTest(data=data), self.assertRaises(catalog.CatalogError):
                catalog.validate(data)

    def test_view_and_index_ordering_are_supported_without_ignoring_semantics(self):
        old = snapshot()
        old['collections'].append({'name': 'view', 'type': 'view', 'uuid': None,
                                  'options': {'viewOn': 'sandbox_usage_events'}, 'indexes': []})
        old['collections'][0]['indexes'].append(interval())
        new = copy.deepcopy(old)
        new['collections'][0]['indexes'].reverse()
        new['collections'].reverse()
        self.assertEqual(catalog.compare(old, new)['outcome'], 'PRESERVED')

    def test_catalog_job_has_no_retries_no_token_and_only_catalog_privileges(self):
        objects = manifests('api', 'runtime-password', 'admin-password', 'key', 'mongo', {})
        catalog.infrastructure(objects, 'cma-migration', 'synthetic-catalog-password')
        self.assertFalse(any(obj['kind'] == 'Secret' for obj in objects))
        init = next(obj for obj in objects if obj['kind'] == 'ConfigMap'
                    and obj['metadata']['name'] == 'mongo-init')['data']['init.sh']
        role = init[init.index('role:"matrix_catalog"'):]
        self.assertIn('actions:["listCollections","listIndexes","killCursors"]', role)
        self.assertNotIn('"find"', role)
        job = catalog.job('cma-migration', 'pinned-local-mongo', 'old-write')
        self.assertEqual(job['spec']['backoffLimit'], 0)
        self.assertEqual(job['spec']['activeDeadlineSeconds'], 120)
        self.assertFalse(job['spec']['template']['spec']['automountServiceAccountToken'])
        self.assertNotIn('synthetic-catalog-password', json.dumps(job))
        self.assertNotIn('exec', json.dumps(job))
        with self.assertRaises(catalog.CatalogError):
            catalog.job('cma-migration', 'mongo', 'unknown')

    def test_capture_binds_raw_metadata_to_selected_artifact_without_exec(self):
        with tempfile.TemporaryDirectory() as directory:
            class Campaign:
                output = Path(directory)
                calls = []

                def kubectl(self, args, **kwargs):
                    self.calls.append((args, kwargs))
                    if 'logs' in args:
                        return json.dumps(snapshot())
                    return ''

            campaign = Campaign()
            receipt, data = catalog.capture(campaign, 'cma-migration', 'mongo',
                                            'old-write', 'a' * 40, 'api@sha256:digest')
            self.assertEqual(data, snapshot())
            self.assertEqual(receipt['source'], 'a' * 40)
            self.assertEqual(receipt['collections'], 1)
            self.assertTrue((campaign.output / receipt['path']).is_file())
            self.assertEqual(len(receipt['sha256']), 64)
            self.assertTrue(all('exec' not in args for args, _ in campaign.calls))

    def test_capture_does_not_emit_receipt_for_failed_job_or_bad_log(self):
        with tempfile.TemporaryDirectory() as directory:
            class Campaign:
                output = Path(directory)
                failure = 'wait'

                def kubectl(self, args, **kwargs):
                    if 'wait' in args and self.failure == 'wait':
                        raise RuntimeError('job failed')
                    return 'not json' if 'logs' in args else ''

            campaign = Campaign()
            for failure, error in [('wait', RuntimeError), ('logs', catalog.CatalogError)]:
                campaign.failure = failure
                with self.assertRaises(error):
                    catalog.capture(campaign, 'cma-migration', 'mongo', 'old-write', 'a' * 40, 'api')
                self.assertFalse(list(Path(directory).glob('catalog-*.json')))

    def test_campaign_keeps_upgrade_index_through_rollback_or_fails_and_cleans(self):
        for remove_on_rollback in (False, True):
            with self.subTest(remove_on_rollback=remove_on_rollback):
                with tempfile.TemporaryDirectory() as directory:
                    old = {'source': 'a' * 40, 'image': 'old', 'config': '1' * 64}
                    new = {'source': 'b' * 40, 'image': 'new', 'config': '2' * 64}
                    args = SimpleNamespace(output=Path(directory) / 'run', old_image='old',
                        new_image='new', old_source=old['source'], new_source=new['source'],
                        plan=Path(directory) / 'plan.json', mongo_image='mongo',
                        identity_image='identity', kind='kind', timeout=1,
                        runtime_image='runtime', fixture_command='fixture', inventory='inventory')
                    args.plan.write_text(json.dumps({'schema': 'cma-migration-matrix-plan-v1',
                                                     'fixtures': [1], 'cells': [1]}))

                    class Campaign:
                        plan_sha256 = '3' * 64
                        cluster = 'cma-matrix-test'
                        commands = []
                        diagnostic_errors = []
                        active_reader = {'pod': {'uid': 'bound-reader'}}
                        cleaned = False

                        def __init__(self, output, *_args): self.output = output
                        def alias_image(self, image, _role): return image
                        def create(self, _objects): pass
                        def boot(self, _alias, _source, stage, _config): return {'stage': stage}
                        def fixture(self, _command, stage, *_args): return {'stage': stage}
                        def reader_pods(self): return self.active_reader['pod']
                        def diagnostics(self): pass
                        def cleanup(self): self.cleaned = True

                    def capture(_campaign, _namespace, _mongo, stage, source, image):
                        data = snapshot()
                        if stage != 'old-write' and not (
                                remove_on_rollback and stage == 'old-read-new-write'):
                            data['collections'][0]['indexes'].append(interval())
                        return {'stage': stage, 'source': source, 'image': image}, data

                    with patch.object(runner, 'Campaign', Campaign), \
                         patch.object(runner, 'checked_identity', side_effect=[old, new]), \
                         patch.object(runner, 'checked_dependency', return_value={}), \
                         patch.object(runner, 'checked_identity_dependency', return_value={}), \
                         patch.object(runner.subprocess, 'check_output', return_value='{}'), \
                         patch.object(runner, 'select_ready_api_pod', side_effect=lambda p, *_: p), \
                         patch.object(catalog, 'capture', side_effect=capture):
                        if remove_on_rollback:
                            with self.assertRaisesRegex(runner.RunnerError, 'catalog changed'):
                                runner.run(args)
                        else:
                            runner.run(args)
                    result = json.loads((args.output / 'kind-evidence.private.json').read_text())
                    self.assertTrue(result['cleanup']['cluster_absent'])
                    self.assertEqual(len(result['catalogs']), 4)
                    self.assertEqual(len(result['catalog_comparisons']), 3)
                    self.assertEqual(result['outcome'], 'BLOCKED' if remove_on_rollback else 'UNVERIFIED')
                    self.assertEqual([row['stage'] for row in result['boots']],
                                     ['historical-boot', 'current-boot', 'rollback-boot'])


if __name__ == '__main__':
    unittest.main()
