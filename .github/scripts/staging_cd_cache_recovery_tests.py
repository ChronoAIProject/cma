"""Exercise the installed retention command against changing cache inventories."""
import contextlib
import io
import json
import os
import sys
import unittest
from dataclasses import replace
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import Mock, patch

import yaml

from actions_cache_retention import BUDGET, DEVELOP, Removal, recovery_kind
from actions_cache_sweep import main
from staging_cd_cache_retention_tests import CLOCK, ROOT, Inventory, entry, seed


def base(i, family="aaa-python-1.2.3"):
    return entry(i, f"codeql-overlay-base-database-1-{family}-{'a' * 40}-{i}-1")


class Recovery(unittest.TestCase):
    def execute(self, api, mode, **overrides):
        with TemporaryDirectory() as directory:
            tmp = Path(directory)
            event = tmp / "event"
            path = ".github/workflows/cache-retention.yml"
            repo = "ChronoAIProject/cma"
            event.write_text(json.dumps({"action": "completed", "workflow_run": {
                "status": "completed", "event": "push", "head_branch": "develop",
                "path": ".github/workflows/staging-cd.yml", "head_repository": {"full_name": repo}}}))
            env = dict(GH_TOKEN="fixture", GITHUB_RUN_ID="7", GITHUB_SHA="a"*40, GITHUB_REPOSITORY=repo,
                       GITHUB_REF=DEVELOP, GITHUB_WORKFLOW_REF=f"{repo}/{path}@{DEVELOP}",
                       GITHUB_EVENT_NAME="workflow_run", GITHUB_EVENT_PATH=str(event),
                       GITHUB_STEP_SUMMARY=str(tmp / "summary"), GITHUB_OUTPUT=str(tmp / "output"))
            with patch.dict(os.environ, env | overrides, clear=True), patch.object(sys, "argv", ["sweep", mode]), \
                    patch("actions_cache_sweep.Actions", return_value=api), \
                    contextlib.redirect_stdout(io.StringIO()) as stdout, contextlib.redirect_stderr(io.StringIO()) as stderr:
                rc = main()
            events = [json.loads(line) for line in stdout.getvalue().splitlines()]
            return rc, events[-1], events[:-1], stderr.getvalue(), (tmp / "summary").read_text(), (tmp / "output").read_text()

    def test_preview_warns_but_succeeds_and_grants_no_write(self):
        api = Inventory(base(1), base(2), entry(3, "unknown", size=BUDGET - 15))
        code, result, events, warnings, summary, outputs = self.execute(api, "--preview-recovery")
        self.assertEqual((code, result["status"], result["after_bytes"]), (0, "preview", None))
        self.assertEqual((result["observed_bytes"], result["projected_bytes"]), (BUDGET + 5, BUDGET - 5))
        self.assertEqual([(e["action"], e["id"], e["successor"]) for e in events], [("would-delete", 1, 2)])
        self.assertIn("::warning::", warnings)
        self.assertIn("projected headroom is 5 bytes", summary)
        self.assertIn("observed_bytes_to_admit=6", outputs)
        api.delete.assert_not_called()

    def test_recount_distinguishes_recovery_from_concurrent_incoming_bytes(self):
        for increase, expected, code in ((0, "recovered", 0), (5, "target-unmet", 3)):
            with self.subTest(increase=increase):
                rows = (base(1), base(2), entry(3, "buildkit-blob-opaque", size=BUDGET - 15))
                api = Inventory(*rows)
                def delete(i):
                    del api.rows[i]
                    api.rows[4] = entry(4, "incoming", size=increase)
                    return True
                api.delete.side_effect = delete
                rc, result, events, _, summary, outputs = self.execute(api, "--apply-recovery")
                self.assertEqual((rc, result["status"], result["after_bytes"]), (code, expected, BUDGET - 5 + increase))
                self.assertEqual(result["projected_bytes"], BUDGET - 5)
                self.assertEqual(result["observed_bytes_to_admit"], int(increase > 0))
                self.assertEqual([e["reason"] for e in events], ["pr-overlay-base"] * 2)
                self.assertIn("shared_buildkit_blobs", summary)
                self.assertIn("status=" + expected, outputs)
                self.assertEqual(api.require_live_policy.call_count, 2)
                api.readers.assert_not_called()

    def test_cross_family_ref_version_empty_or_older_successor_is_ineligible(self):
        old = base(2)
        for keep in (None, old, base(1), replace(base(3), version="another"),
                     replace(base(3), ref="refs/pull/8/merge"), replace(base(3), size=0),
                     base(3, "aaa-javascript-1.2.3"), seed(3, "am")):
            self.assertIsNone(recovery_kind(Removal(old, "superseded", keep), CLOCK.date()))
        newer = base(3)
        for change in ({"version": "new"}, {"accessed": newer.created}):
            api = Inventory(old, newer)
            api.visible = Mock(side_effect=[newer, replace(old, **change)])
            _, result, *_ = self.execute(api, "--apply-recovery")
            self.assertEqual(result["held_by_reason"]["candidate-or-successor-changed"], old.size)
            api.delete.assert_not_called()

    def test_keeps_current_base_shared_layers_and_other_namespaces(self):
        oldest, newest = base(1), base(2)
        legacy = replace(seed(3, "am"), key=seed(3, "am").key.replace("-v4-", "-v3-"))
        protected = [legacy, base(4, "bbb-python-1.2.3"), replace(base(5), version="other"),
                     entry(6, "buildkit-blob-opaque"), replace(oldest, id=7, ref="refs/pull/8/merge"),
                     entry(8, "index-cma-web-1-fd3dfe2c#1"), entry(9, "index-cma-web-1-fd3dfe2c#2")]
        api = Inventory(oldest, newest, *protected)
        rc, result, *_ = self.execute(api, "--apply-recovery")
        self.assertEqual((rc, result["status"]), (0, "under-target"))
        self.assertEqual(set(api.rows), {newest.id, *(r.id for r in protected)})
        api.delete.assert_called_once_with(oldest.id)
        # Protected develop/release jobs do not consume the managed PR base.
        # A PR racing this deletion can fall back to a full analysis.
        api.readers.assert_not_called()
        for workflow in (ROOT / ".github/workflows").glob("*.yml"):
            self.assertNotIn("codeql-overlay-base-", workflow.read_text())

    def test_missing_reader_proof_and_api_failures_remain_red(self):
        api = Inventory(seed(1, "am"), seed(2, "am"))
        with patch("actions_cache_sweep.cargo_reader_hold", return_value="reader-history-incomplete"):
            rc, result, _, notices, *_ = self.execute(api, "--apply-recovery")
        self.assertEqual((rc, result["status"]), (1, "proof-incomplete"))
        api.delete.assert_not_called()
        self.assertIn("::error::", notices)
        for failure in ("entries", "require_live_policy", "delete", "recount"):
            api = Inventory(base(1), base(2))
            if failure == "recount":
                api.entries = Mock(side_effect=[list(api.rows.values()), ValueError("invalid inventory")])
            else:
                setattr(api, failure, Mock(side_effect=RuntimeError("operation refused")))
            rc, result, _, notices, _, outputs = self.execute(api, "--apply-recovery")
            self.assertEqual((rc, result["status"]), (1, "execution-error"))
            self.assertNotIn("after_bytes", result)
            self.assertIn("::error::", notices)
            self.assertIn("status=execution-error", outputs)
        rc, result, *_ = self.execute(Inventory(), "--apply-recovery", GITHUB_RUN_ID="invalid")
        self.assertEqual((rc, result["status"]), (1, "execution-error"))

    def test_workflow_exposes_recount_and_runs_preview_with_read_only_token(self):
        jobs = yaml.safe_load((ROOT / ".github/workflows/cache-retention.yml").read_text())["jobs"]
        self.assertEqual(jobs["sweep"]["permissions"], {"actions": "read"})
        self.assertIn("--preview-recovery", jobs["sweep"]["steps"][-1]["run"])
        self.assertEqual(jobs["recover"]["permissions"], {"actions": "write"})
        for name in ("status", "before_bytes", "after_bytes", "observed_bytes_to_admit"):
            self.assertIn("steps.recovery.outputs." + name, jobs["recover"]["outputs"][name])
