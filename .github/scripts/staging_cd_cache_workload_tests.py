"""Only a complete idle staging attempt suppresses cache maintenance."""
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock

import yaml

from actions_cache_workload import CONTROL_OUTCOMES, PROVEN_SKIP, idle_attempt, needs_maintenance
from staging_cd_common import Refusal

ROOT = Path(__file__).resolve().parents[2]
SOURCE = "a" * 40
REPO = "ChronoAIProject/cma"
RUN = {"id": 42, "run_attempt": 2, "head_sha": SOURCE, "status": "completed", "conclusion": "success",
       "event": "push", "head_branch": "develop", "head_repository": {"full_name": REPO},
       "path": ".github/workflows/staging-cd.yml"}


def idle_inventory(*, allocate="success"):
    conclusions = dict.fromkeys(PROVEN_SKIP, "skipped")
    conclusions.update({"plan": "success", "allocate": allocate, "Publish journey probe": "success",
                        "Report component outcomes": "success"})
    jobs = [{"id": index, "name": name, "run_id": RUN["id"], "run_attempt": RUN["run_attempt"],
             "head_sha": SOURCE, "status": "completed", "conclusion": conclusions[name]}
            for index, name in enumerate(sorted(conclusions), 1)]
    return {"total_count": len(jobs), "jobs": jobs}


class CacheWorkloadTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.event_path = Path(temp.name) / "event.json"
        self.env = {"GITHUB_EVENT_NAME": "workflow_run", "GITHUB_REF": "refs/heads/develop", "GITHUB_REPOSITORY": REPO,
                    "GITHUB_WORKFLOW_REF": f"{REPO}/.github/workflows/cache-retention.yml@refs/heads/develop",
                    "GITHUB_EVENT_PATH": str(self.event_path), "GH_TOKEN": "fixture"}

    def event(self, **changes):
        self.event_path.write_text(json.dumps({"action": "completed", "workflow_run": RUN | changes}))

    def test_complete_idle_attempt_uses_one_bound_inventory(self):
        self.event()
        client = Mock()
        client.return_value.request.return_value = idle_inventory()
        self.assertFalse(needs_maintenance(self.env, client))
        client.assert_called_once_with("fixture")
        request = client.return_value.request
        request.assert_called_once()
        self.assertEqual(request.call_args.args, ("actions/runs/42/attempts/2/jobs?per_page=100",))
        self.assertEqual(request.call_args.kwargs["attempts"], 2)

    def test_allocation_handoff_is_idle_only_if_no_component_job_ran(self):
        self.event()
        client = Mock()
        inventory = idle_inventory(allocate="skipped")
        client.return_value.request.return_value = inventory
        self.assertFalse(needs_maintenance(self.env, client))

        release = next(job for job in inventory["jobs"] if job["name"] == "Release web")
        release.update(name="Release web / Publish", conclusion="failure")
        self.assertTrue(needs_maintenance(self.env, client))

    def test_activity_incomplete_or_different_attempt_preserves_maintenance(self):
        def check(inventory):
            self.assertFalse(idle_attempt(inventory, 42, 2, SOURCE))

        for name, job_changes in (
            ("infrastructure", {"conclusion": "success"}),
            ("Publish protected Web", {"conclusion": "success"}),
            ("Publish protected Web", {"name": "Publish protected Web / Protect Web rollout / Publish", "conclusion": "success"}),
            ("Publish journey probe", {"conclusion": "failure"}),
            ("Release am", {"name": "Release am / Publish", "conclusion": "success"}),
            ("Release eg", {"run_attempt": 1}),
            ("Promote web", {"run_id": True}),
            ("Promote runtime", {"head_sha": "b" * 40}),
            ("Report component outcomes", {"conclusion": "failure"}),
            ("allocate", {"status": "in_progress"}),
        ):
            with self.subTest(job=name, changes=job_changes):
                inventory = idle_inventory()
                job = next(job for job in inventory["jobs"] if job["name"] == name)
                job.update(job_changes)
                check(inventory)

        headless = idle_inventory()
        headless["jobs"].pop()
        check(headless)
        headless["total_count"] -= 1
        check(headless)
        duplicate = idle_inventory()
        duplicate["jobs"][0]["id"] = duplicate["jobs"][1]["id"]
        check(duplicate)
        future = idle_inventory()
        future["jobs"].append(future["jobs"][-1] | {"id": 99, "name": "Future cache writer", "conclusion": "success"})
        future["total_count"] += 1
        check(future)
        self.event()
        client = Mock()
        client.return_value.request.side_effect = Refusal("GitHub API request failed")
        self.assertTrue(needs_maintenance(self.env, client))

    def test_failures_manual_and_other_workflows_need_no_activity_lookup(self):
        client = Mock()
        for changes in ({"conclusion": "failure"}, {"path": ".github/workflows/ci-omarchy.yml"},
                        {"path": ".github/workflows/migration-harness.yml"}, {"head_sha": "bad"},
                        {"run_attempt": None}, {"head_repository": {"full_name": "other/cma"}}):
            with self.subTest(changes=changes):
                self.event(**changes)
                self.assertTrue(needs_maintenance(self.env, client))
        self.event()
        self.assertTrue(needs_maintenance(self.env | {"GITHUB_EVENT_NAME": "workflow_dispatch"}, client))
        client.assert_not_called()

    def test_activity_contract_and_failed_classifier_keep_existing_authority(self):
        workflow_dir = ROOT / ".github/workflows"
        stage = yaml.safe_load((workflow_dir / "staging-cd.yml").read_text())["jobs"]
        self.assertEqual(CONTROL_OUTCOMES.keys() | PROVEN_SKIP, {job.get("name", key) for key, job in stage.items()})
        retention = yaml.safe_load((workflow_dir / "cache-retention.yml").read_text())["jobs"]
        self.assertEqual(retention["staging_activity"]["permissions"], {"actions": "read"})
        self.assertIn("workflow_run.path == '.github/workflows/staging-cd.yml'", retention["staging_activity"]["if"])
        for name in ("sweep", "recover"):
            job = retention[name]
            self.assertEqual(job["needs"], "staging_activity")
            self.assertIn("!cancelled()", job["if"])
            self.assertIn("needs.staging_activity.result != 'success' || needs.staging_activity.outputs.needed != 'false'",
                          job["if"])
        self.assertNotIn("needs", retention["recover_native_buildkit"])
        self.assertIn("github.event_name == 'workflow_dispatch'", retention["sweep"]["if"])
