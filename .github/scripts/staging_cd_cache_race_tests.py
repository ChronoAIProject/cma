import contextlib
import io
import os
import sys
import unittest
from dataclasses import replace
from datetime import datetime, timedelta, timezone
from unittest.mock import Mock, patch

import yaml

from actions_cache_run_history import ACTIVE_HORIZON, IncompleteHistory, cargo_reader_hold
from actions_cache_sweep import Actions, main, sweep
from staging_cd_cache_retention_tests import DIGEST, GRAPH, Inventory, POLICY, ROOT, entry, seed


class ExpiredSeeds(unittest.TestCase):
    def setUp(self):
        self.enterContext(contextlib.redirect_stdout(io.StringIO()))
        self.enterContext(patch("actions_cache_run_history.FIRST_POSSIBLE_V4_RUN", 40))
        self.now = datetime.now(timezone.utc)
        self.yesterday = self.now - timedelta(days=1)
        self.old = replace(seed(3, "am", family=GRAPH), created=self.yesterday)
        self.new = replace(seed(4, "am"), created=self.now)

    def inventory(self, *extra, completed=True):
        api = Inventory(self.old, self.new, *extra)
        old = {"id": 21, "run_number": 40, "created_at": self.yesterday.isoformat(),
               "path": ".github/workflows/staging-cd.yml", "head_branch": "develop", "event": "push",
               "run_attempt": 2, "head_repository": {"full_name": "ChronoAIProject/cma"},
               "status": "completed" if completed else "pending"}
        api.list = Mock(return_value=[old, old | {"run_number": 41, "id": 22,
                                      "created_at": self.now.isoformat(), "status": "in_progress"}])
        return api

    def test_status_change_cannot_hide_overnight_reader(self):
        api = self.inventory(completed=False)
        for status in ("pending", "in_progress", "waiting", "queued"):
            api.list.return_value[0]["status"] = status
            self.assertEqual(sweep(api, POLICY, True, recovery_only=True)["deleted_bytes"], 0)
        api.delete.assert_not_called()
        for call in api.list.call_args_list:
            self.assertNotIn("status", call.kwargs)
            self.assertIn("created", call.kwargs)
        api.list.return_value[0]["status"] = "completed"
        self.assertEqual(sweep(api, POLICY, True, recovery_only=True)["deleted_bytes"], self.old.size)
        api.delete.assert_called_once_with(self.old.id)
        self.assertIn(self.new.id, api.rows)

    def test_consistent_total_cannot_hide_a_missing_or_pending_reader(self):
        api = self.inventory()
        old, after = api.list.return_value
        after = after | {"id": 23, "run_number": 42}
        reader = old | {"id": 22, "run_number": 41, "status": "pending"}
        for rows in ([old, after], [old, reader, after], [reader, after], [old, reader]):
            api.list.return_value = rows
            self.assertEqual(sweep(api, POLICY, True, recovery_only=True)["deleted_bytes"], 0)
            api.delete.assert_not_called()
        api.list.return_value = [after, reader | {"status": "completed"}, old]
        sweep(api, POLICY, True, recovery_only=True)
        api.delete.assert_called_once_with(self.old.id)

    def test_expired_original_and_same_number_with_conflicting_identity(self):
        api = self.inventory()
        original, after = api.list.return_value
        api.list.return_value = [original, original | {"id": 300}, after]
        self.assertTrue(cargo_reader_hold(api, self.yesterday.date(), self.now))
        self.assertIsNone(cargo_reader_hold(api, (self.now - ACTIVE_HORIZON - timedelta(days=1)).date(), self.now))
        api.list.return_value = [original | {"run_number": 99, "created_at":
                                 (self.now - ACTIVE_HORIZON - timedelta(days=1)).isoformat()},
                                 after | {"run_number": 100}]
        self.assertIsNone(cargo_reader_hold(api, self.yesterday.date(), self.now))

    def test_only_superseded_expired_develop_seeds_are_mutated(self):
        same_day = replace(seed(7, "am", graph=DIGEST), created=self.now + timedelta(seconds=2))
        pr = replace(self.old, id=12, ref="refs/pull/19/merge")
        api = self.inventory(pr, same_day, entry(21, "index-cma-web-1-fd3dfe2c#8"),
                             entry(22, "index-cma-web-1-fd3dfe2c#9"))
        legacy = replace(self.old, id=23, key=self.old.key.replace("-v4-", "-v3-"))
        api.rows[legacy.id] = legacy
        sweep(api, POLICY, True, recovery_only=True)
        api.delete.assert_called_once_with(self.old.id)
        self.assertEqual(set(api.rows), {4, 7, 12, 21, 22, 23})
        api = self.inventory()
        del api.rows[self.new.id]
        self.assertEqual(sweep(api, POLICY, True, recovery_only=True)["deleted_bytes"], 0)
        api.delete.assert_not_called()

    def test_truncation_missing_replacement_and_invalid_run_hold_seed(self):
        api = self.inventory()
        api.list.side_effect = IncompleteHistory("truncated")
        self.assertEqual(sweep(api, POLICY, True, recovery_only=True)["deleted_bytes"], 0)
        api.list.side_effect = None
        api.list.return_value[0]["head_branch"] = "feature"
        self.assertTrue(cargo_reader_hold(api, self.yesterday.date(), self.now))
        api.visible = Mock(return_value=None)
        self.assertEqual(sweep(api, POLICY, True, recovery_only=True)["deleted_bytes"], 0)
        api.delete.assert_not_called()
        platform = Actions("fixture", 100)
        platform.api.request = Mock(return_value={"total_count": 1000, "workflow_runs": []})
        self.assertTrue(cargo_reader_hold(platform, self.yesterday.date(), self.now))
        platform.api.request.assert_called_once()

    def test_scoped_preview_matches_reader_and_replacement_checks_without_mutation(self):
        api = self.inventory(completed=False)
        self.assertEqual(sweep(api, POLICY, recovery_only=True)["planned_bytes"], 0)
        api.list.return_value[0]["status"] = "completed"
        preview = sweep(api, POLICY, recovery_only=True)
        self.assertEqual((preview["planned_bytes"], preview["deleted_bytes"]), (self.old.size, 0))
        api.delete.assert_not_called()

    def test_only_develop_completion_can_enable_scoped_pruning(self):
        job = yaml.safe_load((ROOT / ".github/workflows/cache-retention.yml").read_text())["jobs"]["recover"]
        self.assertEqual(job["permissions"], {"actions": "write"})
        self.assertIn("github.event_name == 'workflow_run'", job["if"])
        self.assertIn("vars.CMA_LEGACY_BUILDKIT_DRAIN != 'true'", job["if"])
        self.assertIn("--apply-recovery", job["steps"][-1]["run"])
        with patch.object(sys, "argv", ["report", "--apply-recovery"]), \
                patch.dict(os.environ, {"GITHUB_EVENT_NAME": "workflow_dispatch"}, clear=True), \
                patch("actions_cache_sweep.Actions") as client, contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit):
                main()
            client.assert_not_called()
