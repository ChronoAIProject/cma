"""An unrelated develop push can promote a final until the controller verifies it."""
import copy
import json
import unittest
from unittest.mock import Mock, patch

from staging_cd_allocate import allocate_queue
from staging_cd_common import Refusal
from staging_cd_promotion import PAGE_SIZE, PREFIX, available_proofs, inventory, proof, snapshot
from staging_cd_queue_tests import FIRST, SECOND, THIRD
from staging_cd_history import DevelopHistory
from staging_cd_receipts_tests import fixture_receipt, fixture_tag
from staging_cd_set import private_document, release_set
from staging_cd_versions import REPOSITORY

ENVIRONMENT = "staging"


class PromotionTests(unittest.TestCase):
    def setUp(self):
        self.receipt = fixture_receipt(component="web", source=SECOND)
        self.request = private_document(release_set({"web": self.receipt}))["components"]
        self.stamp = proof(self.request)
        self.rows, self.api_rows = [], {}

    def writer(self, number, *, name="Promote web / Deploy admitted components / Deploy set",
               stamp=None, status="completed", conclusion="success", manual=False):
        self.rows.insert(0, {"id": number, "sha": THIRD, "ref": "develop", "environment": ENVIRONMENT,
                             "created_at": f"2026-11-21T13:{number:02d}:00Z"})
        run_id, job_id = number + 10, number + 20
        self.api_rows[f"deployments/{number}/statuses?per_page=6"] = [
            {"state": "inactive"}, {"state": conclusion, "log_url": f"https://github.com/{REPOSITORY}/actions/runs/{run_id}/job/{job_id}"}]
        job = {"id": job_id, "name": name, "status": status, "conclusion": conclusion, "run_id": run_id,
               "run_attempt": 1, "head_sha": THIRD,
               "steps": [{"name": "Coordinate the immutable deployment set", "conclusion": conclusion}]}
        if stamp is not None:
            job["steps"].append({"name": PREFIX + stamp, "conclusion": "success"})
        self.api_rows[f"actions/jobs/{job_id}"] = job
        repo = {"full_name": REPOSITORY}
        run = {"path": ".github/workflows/" + ("redeploy-staging.yml" if manual else "staging-cd.yml"),
               "id": run_id, "run_attempt": 1, "event": "workflow_dispatch" if manual else "push",
               "repository": repo, "head_repository": repo, "head_sha": THIRD, "head_branch": "develop"}
        self.api_rows[f"actions/runs/{run_id}/attempts/1"] = run
        return job, run

    def plan(self, *, source=SECOND, event=THIRD, receipt=None, entries=None, pending=(), read_tags=None):
        api, store = Mock(), Mock()
        api.history.return_value = DevelopHistory(THIRD, [THIRD, SECOND, FIRST])
        api.tags.return_value = [fixture_tag(receipt or self.receipt)]
        api.tags.side_effect = read_tags
        store.list.return_value = list(pending)
        api.request.return_value = {"id": 88, "run_attempt": 1, "event": "push", "head_branch": "develop",
                                    "repository": {"full_name": REPOSITORY}, "status": "in_progress"}
        queue = {"head": THIRD, "event_source": event, "changes": [
            {"source": source, "tree": self.receipt["identity"]["tree"], "deployables": ["cma-web"]}]
            if entries is None else entries}
        matrix = allocate_queue(queue, api, store, 99, 1, read_proofs=lambda: inventory(self.read, ENVIRONMENT))
        store.insert.assert_not_called()
        if not pending:
            api.request.assert_not_called()
        return [task for row in matrix["include"] for task in json.loads(row["tasks"])]

    def test_cancelled_final_is_promoted_by_next_merge_until_verified(self):
        self.writer(1, conclusion="cancelled")
        self.assertEqual(self.plan(), [{"component": "web", "mode": "resume", "receipt": self.receipt}])
        self.writer(2, stamp=self.stamp)
        self.assertEqual(self.plan(), [])
        self.writer(3, name="Promote eg / Deploy admitted components / Deploy set", conclusion="failure")
        self.writer(4, name="infrastructure")
        self.assertEqual(self.plan(), [])
        self.writer(5, manual=True, name="deploy / Deploy set", conclusion="failure")
        self.assertEqual(self.plan()[0]["receipt"], self.receipt)

    def test_positive_evidence_must_bind_the_current_source_version_and_digest(self):
        self.writer(1, stamp=self.stamp)
        for number, field in enumerate(("source_sha", "version", "image"), 2):
            changed = copy.deepcopy(self.request)
            changed["web"][field] += "different"
            self.writer(number, stamp=proof(changed))
            with self.subTest(field=field):
                self.assertEqual(self.plan()[0]["mode"], "resume")
            self.rows.pop(0)

    def test_no_recovery_for_stale_event_or_source_outside_the_adoption_window(self):
        self.assertEqual(self.plan(event=FIRST), [])
        self.assertEqual(self.plan(entries=[]), [])

    def test_active_owner_prevents_recovery_even_with_no_verification(self):
        owner = dict(self.receipt["identity"], run_id=88, run_attempt=1, schema="cma-version-reservation-v1")
        del owner["build"]
        self.assertEqual(self.plan(pending=[owner]), [])

    def test_controller_can_finish_while_allocation_reads_publication_inventory(self):
        job, _ = self.writer(1, stamp=self.stamp, status="in_progress")
        self.assertEqual(inventory(self.read, ENVIRONMENT), {})

        def read_tags():
            job["status"] = "completed"
            return [fixture_tag(self.receipt)]

        self.assertEqual(self.plan(read_tags=read_tags), [])

    def test_automatic_retry_is_scoped_and_can_supply_controller_proof(self):
        self.writer(1, stamp=self.stamp)
        self.writer(2, name="Promote eg / Deploy admitted components again / Deploy set", conclusion="failure")
        self.assertEqual(self.plan(), [])
        self.writer(3, name="Promote web / Deploy admitted components again / Deploy set", stamp=self.stamp)
        self.assertEqual(self.plan(), [])

    def test_observation_failures_schedule_existing_finals_without_conveying_authority(self):
        for result in (None, OSError("unavailable")):
            with patch("staging_cd_promotion.read_snapshot", return_value=result,
                       side_effect=result if isinstance(result, Exception) else None):
                self.assertEqual(available_proofs("fixture", ENVIRONMENT), {})
        self.assertEqual(self.plan()[0]["receipt"], self.receipt)

    def test_writer_must_belong_to_current_first_parent_history(self):
        job, run = self.writer(1, stamp=self.stamp)
        job["head_sha"] = run["head_sha"] = self.rows[0]["sha"] = "e" * 40
        self.assertEqual(self.plan()[0]["mode"], "resume")

    def read(self, path):
        if path.startswith("deployments?"):
            count, _, page = path.partition("per_page=")[2].partition("&page=")
            size, index = int(count), int(page or "1") - 1
            return self.rows[index * size:(index + 1) * size]
        return self.api_rows[path]

    def test_snapshot_names_unfinished_and_covered_writers_and_the_listing_age(self):
        self.writer(1, stamp=self.stamp)
        self.writer(2, name="Promote eg / Deploy admitted components / Deploy set", status="in_progress")
        self.writer(3, name="Promote contract / Deploy admitted components / Deploy set", conclusion="failure")
        value = snapshot(self.read, ENVIRONMENT)
        self.assertEqual(value.proofs, {"web": {"writer": THIRD, "stamp": self.stamp}})
        self.assertEqual((value.observed, value.running), ({"am", "eg", "runtime", "runtime-omarchy", "web"}, {"eg"}))
        # Fewer rows than the listing holds are every deployment there is.
        self.assertIsNone(value.since)
        self.assertEqual(inventory(self.read, ENVIRONMENT), value.proofs)
        for number in range(4, PAGE_SIZE + 1):
            self.writer(number, name="infrastructure")
        value = snapshot(self.read, ENVIRONMENT)
        self.assertEqual((value.observed, value.since), ({"am", "eg", "runtime", "runtime-omarchy", "web"},
                                                         "2026-11-21T13:01:00Z"))

    def test_unrelated_newer_deployments_do_not_evict_an_older_web_proof(self):
        self.writer(1, stamp=self.stamp)
        for number in range(2, 2 * PAGE_SIZE + 2):
            self.writer(number, name="infrastructure")
        self.assertEqual(self.plan(), [])
        value = snapshot(self.read, ENVIRONMENT)
        self.assertEqual(value.proofs, {"web": {"writer": THIRD, "stamp": self.stamp}})
        self.assertIsNone(value.since)

    def test_read_failure_on_an_older_page_keeps_the_fenced_prefix_only(self):
        for number in range(1, PAGE_SIZE + 1):
            self.writer(number, name="infrastructure")
        self.writer(PAGE_SIZE + 1, stamp=self.stamp)

        def read(endpoint):
            if endpoint.endswith("&page=2"):
                raise OSError("scan deadline")
            return self.read(endpoint)

        fence = Mock(side_effect=self.read)
        value = snapshot(read, ENVIRONMENT, fence_request=fence)
        self.assertEqual(value.proofs, {"web": {"writer": THIRD, "stamp": self.stamp}})
        self.assertEqual(value.observed, {"web"})
        self.assertEqual(value.since, "2026-11-21T13:02:00Z")
        fence.assert_called_once_with("deployments?environment=staging&per_page=1")
        fence.side_effect = None
        fence.return_value = [{"id": PAGE_SIZE + 2}]
        self.assertIsNone(snapshot(read, ENVIRONMENT, fence_request=fence))

    def test_page_offset_change_cannot_hide_a_newer_writer(self):
        self.writer(1, stamp=self.stamp)
        self.writer(2, conclusion="failure")
        for number in range(3, PAGE_SIZE + 3):
            self.writer(number, name="infrastructure")

        def read(endpoint):
            if endpoint.endswith("&page=2"):
                self.rows.pop(1)  # The newest ID stays put, but the offset moves.
            return self.read(endpoint)

        value = snapshot(read, ENVIRONMENT)
        self.assertEqual((value.proofs, value.observed), ({}, set()))
        self.assertEqual(value.since, "2026-11-21T13:03:00Z")

    def test_new_deployment_during_evidence_read_invalidates_snapshot(self):
        self.writer(1, stamp=self.stamp)
        def read(path):
            if path.startswith("deployments?"):
                return [{"id": 2}] if path.endswith("=1") else self.rows
            return self.api_rows[path]
        self.assertEqual(inventory(read, ENVIRONMENT), {})
        self.assertIsNone(snapshot(read, ENVIRONMENT))

    def test_untrusted_run_or_newer_unidentified_writer_cannot_suppress_promotion(self):
        job, run = self.writer(1, stamp=self.stamp)
        run["event"] = "pull_request"
        self.assertEqual(self.plan()[0]["receipt"], self.receipt)
        run["event"] = "push"
        self.writer(2, name="Unidentified writer")
        self.assertEqual(self.plan()[0]["receipt"], self.receipt)

    def test_a_cancelled_writer_cannot_confirm_a_final_even_after_coordinating(self):
        job, _ = self.writer(1, stamp=self.stamp, conclusion="cancelled")
        job["steps"][0]["conclusion"] = "success"
        self.assertEqual(self.plan()[0]["mode"], "resume")
        self.assertEqual(snapshot(self.read, ENVIRONMENT).proofs, {})
        job["conclusion"] = "success"
        self.assertEqual(self.plan(), [])

    def test_unknown_or_running_outcome_cannot_confirm_a_final(self):
        self.writer(1, stamp=self.stamp)
        job, _ = self.writer(2, stamp=self.stamp, status="in_progress")
        self.assertEqual(self.plan()[0]["mode"], "resume")
        job["status"] = "completed"
        job["steps"][0]["conclusion"] = "skipped"
        self.assertEqual(self.plan()[0]["mode"], "resume")
        self.rows.reverse()
        with self.assertRaisesRegex(Refusal, "order"):
            self.plan()
