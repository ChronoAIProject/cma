import json
import unittest
from dataclasses import replace
from datetime import timedelta
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import Mock

import yaml

from actions_cache_buildkit_recovery import authorized, recover
from actions_cache_retention import BUDGET
from staging_cd_buildkit_cache_inventory_tests import blob, index
from staging_cd_cache_retention_tests import CLOCK, Inventory, ROOT, entry


class BuildKitInventory(Inventory):
    def __init__(self, *rows):
        super().__init__(*rows)
        self.active_runs = Mock(return_value=[])

    def entries(self, key=None, ref=None, **_):
        return [row for row in self.rows.values()
                if (key is None or row.key.startswith(key)) and (ref is None or row.ref == ref)]


class NativeRecovery(unittest.TestCase):
    def setUp(self):
        self.idx = index(1)
        self.reachable = replace(blob(2, "a"), size=50)
        self.orphan = replace(blob(3, "b"), size=100)
        self.other_orphan = replace(blob(4, "c"), size=10)
        self.unrelated = entry(5, "unrelated", size=BUDGET - 100)
        self.api = BuildKitInventory(self.idx, self.reachable, self.orphan, self.other_orphan, self.unrelated)
        self.read_index = Mock(return_value={self.reachable.key})

    def run_recovery(self):
        return recover(self.api, self.read_index, "a" * 40, CLOCK + timedelta(hours=48))

    def test_recovery_stops_below_target_and_retains_all_indexes(self):
        result = self.run_recovery()
        self.assertEqual(result['status'], 'recovered')
        self.assertEqual(result['confirmed_deleted_bytes'], self.orphan.size)
        self.assertLess(result['after_bytes'], BUDGET)
        self.api.delete.assert_called_once_with(self.orphan.id)
        self.assertEqual(set(self.api.rows), {1, 2, 4, 5})

    def test_active_reader_or_changed_observation_holds_candidates(self):
        self.api.active_runs.return_value = [{'id': 12}]
        self.assertEqual(self.run_recovery()['status'], 'reader-held')
        self.read_index.assert_not_called()
        self.api.active_runs.return_value = []
        self.api.visible = Mock(return_value=replace(self.orphan, accessed=CLOCK + timedelta(hours=1)))
        self.assertEqual(self.run_recovery()['status'], 'proof-changed')
        self.api.delete.assert_not_called()

    def test_lost_delete_reply_requires_recount_and_never_deletes_again(self):
        def uncertain(cache_id):
            self.api.rows.pop(cache_id)
            raise RuntimeError('lost synthetic reply')
        self.api.delete.side_effect = uncertain
        result = self.run_recovery()
        self.assertEqual(result['status'], 'mutation-uncertain')
        self.assertTrue(result['uncertain_id_absent'])
        self.assertEqual(result['confirmed_deleted_bytes'], 0)
        self.assertLess(result['after_bytes'], BUDGET)
        self.api.delete.assert_called_once_with(self.orphan.id)

    def test_missing_index_never_reaches_delete(self):
        self.read_index.side_effect = ValueError('missing synthetic index')
        result = self.run_recovery()
        self.assertEqual(result['status'], 'proof-incomplete')
        self.assertEqual(result['before_bytes'], result['after_bytes'])
        self.api.delete.assert_not_called()

    def test_all_retained_index_generations_protect_cross_scope_layers(self):
        older = index(6)
        self.api.rows[older.id] = older
        self.read_index.side_effect = lambda key, _: ({self.orphan.key} if key == older.key else {self.reachable.key})
        result = self.run_recovery()
        self.assertEqual(result['status'], 'target-unmet')
        self.api.delete.assert_called_once_with(self.other_orphan.id)
        self.assertEqual(set(self.api.rows), {1, 2, 3, 5, 6})

    def test_mutation_requires_both_flags_and_trusted_completion(self):
        repository = 'ChronoAIProject/cma'
        with TemporaryDirectory() as temp:
            path = Path(temp) / 'event.json'
            run = {'status': 'completed', 'event': 'push', 'path': '.github/workflows/staging-cd.yml',
                   'head_branch': 'develop', 'head_repository': {'full_name': repository}}
            path.write_text(json.dumps({'action': 'completed', 'workflow_run': run}))
            env = {'GITHUB_REPOSITORY': repository, 'GITHUB_REF': 'refs/heads/develop',
                   'GITHUB_EVENT_NAME': 'workflow_run', 'GITHUB_EVENT_PATH': str(path),
                   'GITHUB_WORKFLOW_REF': repository + '/.github/workflows/cache-retention.yml@refs/heads/develop',
                   'CD_LEGACY_BUILDKIT_GC': 'enabled', 'CD_LAYER_CACHE_WRITER_FLOOR': 'verified'}
            self.assertTrue(authorized(env))
            for key in ('CD_LEGACY_BUILDKIT_GC', 'CD_LAYER_CACHE_WRITER_FLOOR'):
                self.assertFalse(authorized(env | {key: ''}))
            self.assertFalse(authorized(env | {'GITHUB_EVENT_NAME': 'workflow_dispatch'}))

        jobs = yaml.safe_load((ROOT / '.github/workflows/cache-retention.yml').read_text())['jobs']
        job = jobs['recover_native_buildkit']
        self.assertIn("vars.CMA_LEGACY_BUILDKIT_GC == 'true'", job['if'])
        self.assertIn("vars.CMA_LAYER_CACHE_WRITER_FLOOR == 'true'", job['if'])
        self.assertIn("github.event_name == 'workflow_run'", job['if'])
        self.assertEqual(job['concurrency'], jobs['retain_layers']['concurrency'])
        self.assertEqual(jobs['inspect_buildkit']['permissions']['actions'], 'read')
