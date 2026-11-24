"""Exercise the installed retention command against changing cache inventories."""
import contextlib
import io
import json
import os
import sys
import unittest
from dataclasses import replace
from datetime import timedelta
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import Mock, patch

import yaml

from actions_cache_retention import BUDGET, DEVELOP, Removal, recovery_kind
from actions_cache_sweep import main
from staging_cd_cache_retention_tests import CLOCK, OMARCHY, POLICY, ROOT, Inventory, entry, seed


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
            self.assertIsNone(recovery_kind(Removal(old, "superseded", keep), CLOCK.date(), POLICY))
        newer = base(3)
        for change in ({"version": "new"}, {"accessed": newer.created}):
            api = Inventory(old, newer)
            api.visible = Mock(side_effect=[newer, replace(old, **change)])
            _, result, *_ = self.execute(api, "--apply-recovery")
            self.assertEqual(result["held_by_reason"]["candidate-or-successor-changed"], old.size)
            api.delete.assert_not_called()

    def test_recovery_reclaims_older_trivy_and_obsolete_omarchy_with_visible_replacements(self):
        old_trivy = entry(1, "cache-trivy-2025-12-30", size=20)
        new_trivy = entry(2, "cache-trivy-2025-12-31")
        old_omarchy = entry(3, OMARCHY + "b" * 64, size=30)
        new_omarchy = entry(4, POLICY.omarchy)
        rows = (old_trivy, new_trivy, old_omarchy, new_omarchy,
                entry(5, "buildkit-blob-opaque", size=BUDGET - 25))
        api = Inventory(*rows)
        with patch("actions_cache_sweep.Policies.from_source", return_value=POLICY):
            code, result, events, *_ = self.execute(api, "--apply-recovery")
        self.assertEqual((code, result["status"], result["deleted_bytes"]), (0, "recovered", 50))
        self.assertEqual({(e["id"], e["successor"], e["reason"]) for e in events if e["action"] == "deleted"},
                         {(1, 2, "superseded-trivy-database"), (3, 4, "superseded-omarchy-archive")})
        self.assertEqual(set(api.rows), {2, 4, 5})

    def test_live_recovery_rejects_wrong_day_version_or_omarchy_recipe(self):
        trivy = entry(1, "cache-trivy-2025-12-30")
        omarchy = entry(1, OMARCHY + "b" * 64)
        for old, keep in (
            (trivy, entry(2, "cache-trivy-2026-01-02")),
            (trivy, entry(2, "cache-trivy-2025-99-99")),
            (trivy, entry(2, "cache-trivy-2025-12-29")),
            (trivy, replace(entry(2, "cache-trivy-2025-12-31"), version="different")),
            (replace(omarchy, key=POLICY.omarchy), entry(2, omarchy.key)),
            (omarchy, entry(2, OMARCHY + "c" * 64)),
        ):
            with self.subTest(old=old.key, keep=keep.key):
                self.assertIsNone(recovery_kind(Removal(old, "superseded", keep), CLOCK.date(), POLICY))

    def test_rust_ci_recovery_keeps_two_generations_and_requires_a_newer_reader(self):
        oldest = entry(1, "v0-rust-build-Linux-x64-01234567-aaaaaaaa", size=30)
        spare = entry(2, "v0-rust-build-Linux-x64-01234567-bbbbbbbb")
        newest = replace(entry(3, "v0-rust-build-Linux-x64-01234567-cccccccc"),
                         accessed=CLOCK + timedelta(seconds=10))
        unrelated = entry(4, "v0-rust-lint-Linux-x64-01234567-dddddddd")
        full = entry(5, "buildkit-blob-opaque", size=BUDGET - 55)
        api = Inventory(oldest, spare, newest, unrelated, full)
        code, result, events, *_ = self.execute(api, "--apply-recovery")
        self.assertEqual((code, result["status"], result["deleted_bytes"]), (0, "recovered", 30))
        self.assertEqual([(row["id"], row["successor"], row["reason"]) for row in events if row["action"] == "deleted"],
                         [(oldest.id, newest.id, "superseded-rust-ci-cache")])
        self.assertEqual(set(api.rows), {spare.id, newest.id, unrelated.id, full.id})
        for changed in (replace(newest, accessed=oldest.accessed),
                        replace(newest, key=newest.key.replace("01234567", "76543210")), unrelated):
            self.assertIsNone(recovery_kind(Removal(oldest, "superseded", changed, spare),
                                            CLOCK.date() + timedelta(days=1), POLICY))
        self.assertIsNone(recovery_kind(Removal(oldest, "superseded", newest, spare), CLOCK.date(), POLICY))

    def test_rust_ci_does_not_delete_when_the_spare_is_empty_or_disappears(self):
        for family in ("build", "lint"):
            prefix = f"v0-rust-{family}-Linux-x64-01234567-"
            oldest = entry(1, prefix + "aaaaaaaa", size=30)
            spare = entry(2, prefix + "bbbbbbbb")
            newest = replace(entry(3, prefix + "cccccccc"), accessed=CLOCK + timedelta(seconds=10))
            for missing in (False, True):
                with self.subTest(family=family, missing=missing):
                    api = Inventory(oldest, spare if missing else replace(spare, size=0), newest)
                    if missing:
                        api.entries = Mock(side_effect=[list(api.rows.values()), [oldest, newest]])
                        del api.rows[spare.id]
                    code, result, *_ = self.execute(api, "--apply-recovery")
                    self.assertEqual((code, result["deleted_bytes"]), (0, 0))
                    self.assertIn(oldest.id, api.rows)
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
