import json
import unittest
from dataclasses import replace
from datetime import timedelta
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import Mock

import yaml

from actions_cache_buildkit_inventory import MAX_INDEX_RETIREMENTS
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

    def run_recovery(self, **options):
        return recover(self.api, self.read_index, "a" * 40, CLOCK + timedelta(hours=48), **options)

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

    def add_replacement_indexes(self):
        for ordinal in (6, 7):
            self.api.rows[ordinal] = index(ordinal)
        self.read_index.side_effect = lambda key, _: (
            {self.orphan.key, self.other_orphan.key} if key == self.idx.key else {self.reachable.key})

    def test_index_retirement_is_opt_in_and_then_requires_a_fresh_blob_proof(self):
        self.add_replacement_indexes()
        preview = self.run_recovery()
        self.assertEqual(preview["status"], "target-unmet")
        self.assertEqual(preview["projected_candidate_bytes_after_index_retirement"],
                         self.orphan.size + self.other_orphan.size)
        self.api.delete.assert_not_called()
        result = self.run_recovery(retire_indexes=True)
        self.assertEqual((result["status"], result["confirmed_retired_indexes"]), ("indexes-retired", 1))
        self.api.delete.assert_called_once_with(self.idx.id)
        self.assertIn(self.orphan.id, self.api.rows)
        self.assertIn(self.other_orphan.id, self.api.rows)

        self.api.delete.reset_mock()
        fresh = self.run_recovery(retire_indexes=True)
        self.assertEqual(fresh["status"], "recovered")
        self.api.delete.assert_called_once_with(self.orphan.id)
        self.assertTrue({6, 7} <= self.api.rows.keys())

    def test_retirement_rechecks_readers_and_limits_mutations(self):
        self.add_replacement_indexes()
        self.api.active_runs.side_effect = [[], [{"id": 43}]]
        self.assertEqual(self.run_recovery(retire_indexes=True)["status"], "reader-held")
        self.api.delete.assert_not_called()
        self.api.active_runs.side_effect = None

        old_ids = range(6, 6 + MAX_INDEX_RETIREMENTS)
        for ordinal in old_ids:
            self.api.rows[ordinal] = index(ordinal)
        newest = (6 + MAX_INDEX_RETIREMENTS, 7 + MAX_INDEX_RETIREMENTS)
        for ordinal in newest:
            self.api.rows[ordinal] = index(ordinal)
        old_keys = {self.idx.key, *(self.api.rows[ordinal].key for ordinal in old_ids)}
        self.read_index.side_effect = lambda key, _: (
            {self.orphan.key, self.other_orphan.key} if key in old_keys else {self.reachable.key})
        result = self.run_recovery(retire_indexes=True)
        self.assertEqual(result["confirmed_retired_indexes"], MAX_INDEX_RETIREMENTS)
        self.assertEqual(self.api.delete.call_count, MAX_INDEX_RETIREMENTS)
        self.assertEqual(result["projected_newly_unreachable_bytes"], 0)
        self.assertTrue(set(newest) <= self.api.rows.keys())
        self.assertIn(self.orphan.id, self.api.rows)

        remaining = self.run_recovery(retire_indexes=True)
        self.assertEqual((remaining["status"], remaining["confirmed_retired_indexes"]), ("indexes-retired", 1))
        self.assertIn(self.orphan.id, self.api.rows)
        self.assertEqual(self.run_recovery(retire_indexes=True)["status"], "recovered")
        self.assertTrue(set(newest) <= self.api.rows.keys())

    def test_recovery_retires_indexes_that_together_cover_the_shortfall(self):
        old, shared_a, shared_b, spare, latest = (index(i) for i in (1, 6, 7, 8, 9))
        # The oldest index is eligible but its unique blob cannot cover the gap.
        # The pair can unlock the larger blob only when both are retired.
        self.api = BuildKitInventory(old, shared_a, shared_b, spare, latest, self.orphan,
                                     self.other_orphan, self.reachable, self.unrelated)
        references = {old.key: {self.other_orphan.key},
                      shared_a.key: {self.orphan.key}, shared_b.key: {self.orphan.key},
                      spare.key: {self.reachable.key}, latest.key: {self.reachable.key}}
        self.api.rows[self.unrelated.id] = replace(self.unrelated, size=BUDGET - 120)
        self.read_index.side_effect = lambda key, _: references[key]
        result = self.run_recovery(retire_indexes=True)
        self.assertEqual(result["status"], "indexes-retired")
        self.assertEqual(result["confirmed_retired_indexes"], 2)
        self.assertEqual(result["projected_newly_unreachable_bytes"], self.orphan.size)
        self.assertEqual([call.args[0] for call in self.api.delete.call_args_list], [shared_a.id, shared_b.id])
        self.assertIn(old.id, self.api.rows)
        self.assertIn(self.orphan.id, self.api.rows)
        self.assertTrue({spare.id, latest.id} <= self.api.rows.keys())

    def test_retirement_stops_if_a_replacement_disappears_after_observation(self):
        self.add_replacement_indexes()

        def active_runs():
            if self.api.active_runs.call_count == 2:
                self.api.rows.pop(7)
            return []

        self.api.active_runs.side_effect = active_runs
        result = self.run_recovery(retire_indexes=True)
        self.assertEqual(result["status"], "proof-changed")
        self.api.delete.assert_not_called()

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
        retirement_step = next(step for step in job["steps"] if "CD_LEGACY_BUILDKIT_INDEX_RETIREMENT" in step.get("env", {}))
        self.assertIn("vars.CMA_LEGACY_BUILDKIT_INDEX_RETIREMENT == 'true'",
                      retirement_step["env"]["CD_LEGACY_BUILDKIT_INDEX_RETIREMENT"])

    def test_automatic_inspection_requires_successful_staging_and_observed_pressure(self):
        jobs = yaml.safe_load((ROOT / '.github/workflows/cache-retention.yml').read_text())['jobs']
        sweep, inspect = jobs['sweep'], jobs['inspect_buildkit']
        self.assertEqual(sweep['outputs'], {
            'observed_bytes_to_admit': '${{ steps.preview.outputs.observed_bytes_to_admit }}',
        })
        preview = next(step for step in sweep['steps'] if step.get('id') == 'preview')
        self.assertEqual(preview['run'], 'python3 -B .github/scripts/actions_cache_sweep.py')
        self.assertEqual(inspect['needs'], 'sweep')
        for condition in (
            "github.event_name == 'workflow_dispatch' && inputs.inspect_buildkit",
            "github.event_name == 'workflow_run' && github.event.workflow_run.event == 'push'",
            "github.event.workflow_run.head_repository.full_name == github.repository",
            "github.event.workflow_run.path == '.github/workflows/staging-cd.yml'",
            "github.event.workflow_run.conclusion == 'success'",
            "vars.CMA_LEGACY_BUILDKIT_GC != 'true' && needs.sweep.result == 'success'",
            "fromJSON(needs.sweep.outputs.observed_bytes_to_admit || '0') > 0",
        ):
            self.assertIn(condition, inspect['if'])
        self.assertEqual(inspect['cache-mode'], 'read')
        self.assertEqual(inspect['permissions'], {'actions': 'read', 'contents': 'read'})
        self.assertFalse(inspect['concurrency']['cancel-in-progress'])
        self.assertNotEqual(inspect['concurrency']['group'], jobs['recover_native_buildkit']['concurrency']['group'])
