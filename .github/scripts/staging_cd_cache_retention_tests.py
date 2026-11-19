import contextlib
import io
import json
import os
import subprocess
import sys
import unittest
from dataclasses import replace
from datetime import datetime, timedelta, timezone
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import Mock, patch

import yaml

from actions_cache_retention import BUDGET, DEVELOP, Entry, Policies, Readers, plan
from actions_cache_sweep import Actions, authorized_completion, main, pages, rollover, sweep

ROOT = Path(__file__).resolve().parents[2]
CLOCK = datetime(2026, 1, 1, tzinfo=timezone.utc)
DIGEST, GRAPH = "a" * 64, "b" * 64
OMARCHY = "omarchy-layer-archive-v1-runtime-omarchy-deps-"
POLICY = Policies({service: f"cma-cd-cargo-v3-{service}-{DIGEST}-" for service in ("am", "eg")},
                  OMARCHY + DIGEST)
IDLE = Readers.from_runs([], 100)


def entry(i, key, ref=DEVELOP, size=10):
    return Entry(i, key, ref, "version", size, CLOCK + timedelta(seconds=i), CLOCK)


def seed(i, service, family=DIGEST, graph=GRAPH):
    return entry(i, f"cma-cd-cargo-v3-{service}-{family}-{graph}-{i}-1")


def decisions(rows):
    return {a.entry.id: a.successor.id if a.successor else None for a in plan(rows, POLICY)}


def run(i, event, status="in_progress", pulls=(), branch="develop"):
    return {"id": i, "event": event, "head_branch": branch, "status": status, "pull_requests": [{"number": n} for n in pulls]}


class Inventory:
    def __init__(self, *rows):
        self.run_id = 99
        self.rows = {e.id: e for e in rows}
        self.readers = Mock(return_value=IDLE)
        self.closed_pull = Mock(return_value=True)
        self.require_live_policy = Mock()
        self.sole_rust_writer = Mock(return_value=True)
        self.delete = Mock(side_effect=lambda i: self.rows.pop(i, None) is not None)

    def entries(self):
        return list(self.rows.values())

    def visible(self, item):
        return self.rows.get(item.id)


class Generations(unittest.TestCase):
    def test_scope_versions_and_shared_blobs(self):
        rows = []
        for i, service in enumerate(("am", "eg", "web", "runtime", "runtime-omarchy")):
            prefix = f"index-cma-{service}-1-fd3dfe2c#"
            # Two-digit BuildKit generations sort numerically.
            rows.extend([entry(2*i+1, prefix + "9"), entry(2*i+2, prefix + "10")])
        rows.extend([entry(11, "buildkit-blob-1-sha256:" + DIGEST),
                     replace(rows[1], id=12, version="different"), entry(13, "index-unmanaged")])
        self.assertEqual(decisions(rows), {2*i+1: 2*i+2 for i in range(5)})

    def test_seed_rollover_waits_for_visible_qualified_replacement(self):
        obsolete = entry(1, f"cma-cd-cargo-v2-am-{GRAPH}-12-1")
        previous = seed(2, "am", GRAPH)
        rows = [obsolete, previous, seed(3, "eg"), entry(4, "cma-cd-cargo-v3-am-broken")]
        self.assertEqual(decisions(rows), {})
        self.assertEqual(decisions([previous, seed(8, "am", GRAPH, DIGEST)]), {2: 8})
        rows += [seed(5, "am"), seed(6, "am", graph=DIGEST), seed(7, "am", GRAPH)]
        self.assertEqual(decisions(rows), {1: 6, 2: 6, 5: 6})
        self.assertEqual(decisions([previous, replace(rows[-2], size=0)]), {})

    def test_omarchy_uses_exact_recipe_even_after_historical_rerun(self):
        stale = entry(2, OMARCHY + GRAPH)
        self.assertEqual(decisions([stale]), {})
        self.assertEqual(decisions([entry(1, POLICY.omarchy), stale]), {2: 1})
        self.assertEqual(decisions([entry(1, POLICY.omarchy), replace(stale, version="other")]), {})

    def test_auxiliary_generations_and_ref_isolation(self):
        prefix = "codeql-overlay-base-database-1-aaa-python-1.2.3-"
        keys = [prefix + "0"*40 + "-1-1", prefix + "1"*40 + "-2-1"]
        self.assertEqual(decisions([entry(1, keys[0]), entry(2, keys[1]),
                                    entry(3, keys[0], "refs/heads/other"),
                                    entry(4, keys[0], "refs/pull/9/merge")]), {1: 2, 4: None})

    def test_policy_is_computed_by_current_writers(self):
        policy = Policies.from_source(ROOT)
        self.assertRegex(policy.cargo["am"], r"^cma-cd-cargo-v[23]-am-[a-f0-9]{64}-$")
        self.assertRegex(policy.cargo["eg"], r"^cma-cd-cargo-v[23]-eg-[a-f0-9]{64}-$")
        self.assertRegex(policy.omarchy, "^" + OMARCHY + "[a-f0-9]{64}$")


class Sweeps(unittest.TestCase):
    def setUp(self):
        self.output = io.StringIO()
        self.enterContext(contextlib.redirect_stdout(self.output))
        self.pr = entry(1, "any", "refs/pull/7/merge")
        self.opaque = entry(2, "buildkit-blob-1-sha256:" + DIGEST, size=BUDGET + 5)

    def test_dry_run_and_unrecoverable_byte_shortfall(self):
        api = Inventory(self.pr, self.opaque)
        summary = sweep(api, POLICY)
        api.delete.assert_not_called()
        self.assertEqual(summary["remaining_bytes"], BUDGET + 5)
        self.assertEqual(summary["over_budget_bytes"], 5)
        self.assertEqual(sweep(Inventory(replace(self.opaque, size=BUDGET)), POLICY)["bytes_to_admit"], 1)
        self.assertEqual(json.loads(self.output.getvalue())["action"], "would-delete")

    def test_apply_recounts_concurrent_changes_and_is_idempotent(self):
        api = Inventory(self.pr, self.opaque)
        def remove(i):
            api.rows[9] = entry(9, "unknown", size=500)
            del api.rows[i]
            return True
        api.delete.side_effect = remove
        summary = sweep(api, POLICY, True)
        self.assertEqual(summary["remaining_bytes"], self.opaque.size + 500)
        self.assertEqual(summary["deleted_bytes"], 10)
        self.assertEqual(sweep(api, POLICY, True)["planned_bytes"], 0)
        api.delete.assert_called_once_with(self.pr.id)

    def test_run_starting_during_sweep_retains_its_restores(self):
        rows = [self.pr, seed(3, "eg"), seed(4, "eg")]
        api = Inventory(*rows)
        busy = Readers.from_runs([run(23, "pull_request", pulls=[7]), run(24, "push")], 100)
        api.readers.side_effect = [IDLE, busy]
        self.assertEqual(sweep(api, POLICY, True)["held_candidate_bytes"], 20)
        api.delete.assert_not_called()

    def test_pr_restores_are_best_effort_and_develop_readers_are_protected(self):
        busy = Readers.from_runs([run(23, "pull_request")], 100)
        self.assertTrue(busy.holds(self.pr.ref))
        self.assertFalse(busy.holds(DEVELOP))
        self.assertFalse(Readers.from_runs([run(101, "push", branch="other")], 100).holds(DEVELOP))
        busy = Readers.from_runs([run(23, "push"), run(100, "schedule"), run(25, "pull_request", "completed")], 100)
        self.assertFalse(busy.holds(self.pr.ref))
        self.assertTrue(busy.holds(DEVELOP))
        with self.assertRaises(ValueError):
            Readers.from_runs([run(23, "pull_request", pulls=[0])], 100)

    def test_disappeared_successor_or_recent_access_skips_delete(self):
        for changed in (None, replace(self.pr, accessed=CLOCK + timedelta(seconds=9))):
            api = Inventory(seed(3, "eg"), seed(4, "eg")) if changed is None else Inventory(self.pr)
            api.visible = Mock(return_value=changed)
            sweep(api, POLICY, True)
            api.delete.assert_not_called()

    def test_gone_and_permission_refused_are_distinct(self):
        api = Actions("synthetic", 100)
        for status, code, result in ((204, 0, True), (404, 1, False), (403, 1, None)):
            response = subprocess.CompletedProcess([], code, stdout=f"HTTP/2.0 {status}\n")
            with patch("actions_cache_sweep.subprocess.run", return_value=response):
                if result is None:
                    with self.assertRaises(RuntimeError):
                        api.delete(7)
                else:
                    self.assertIs(api.delete(7), result)
        with self.assertRaises(ValueError):
            api.delete("7")

    def test_open_pr_is_preserved_even_when_idle(self):
        api = Inventory(self.pr)
        api.closed_pull.return_value = False
        self.assertEqual(sweep(api, POLICY, True)["held_candidate_bytes"], self.pr.size)
        api.delete.assert_not_called()

    def test_drift_during_sweep_preserves_new_omarchy_archive(self):
        stale, current = entry(3, POLICY.omarchy), entry(4, OMARCHY + GRAPH)
        api = Inventory(stale, current, self.pr)
        self.assertEqual(decisions(api.entries()), {1: None, 4: 3})
        api.require_live_policy.side_effect = [None, ValueError("develop changed")]
        with self.assertRaisesRegex(ValueError, "develop changed"):
            sweep(api, POLICY, True, "a" * 40)
        api.delete.assert_not_called()

    def test_rerun_starting_after_last_census_cannot_lose_a_reachable_cache(self):
        now = datetime.now(timezone.utc)
        index = "index-cma-web-1-fd3dfe2c#"
        selector = "cma-develop-selection-v2-" + DIGEST + "-"
        api = Inventory(self.pr, replace(seed(3, "eg"), created=now), replace(seed(4, "eg"), created=now),
                        entry(5, index + "9"), entry(6, index + "10"),
                        entry(7, OMARCHY + GRAPH), entry(8, POLICY.omarchy),
                        entry(9, selector + "a"*40 + "-9-1"), entry(10, selector + "b"*40 + "-10-1"))
        def start_reader_then_delete(cache_id):
            api.readers.return_value = Readers.from_runs([run(101, "push")], api.run_id)
            self.assertEqual(cache_id, self.pr.id)
            return api.rows.pop(cache_id) is not None
        api.delete.side_effect = start_reader_then_delete
        result = sweep(api, POLICY, True)
        self.assertEqual(result["legacy_reader_hold_bytes"], 40)
        api.delete.assert_called_once_with(self.pr.id)
        self.assertEqual(set(api.rows), set(range(3, 11)))

    def test_prior_day_seed_can_retire_after_overnight_reader_finishes(self):
        now = datetime.now(timezone.utc)
        api = Inventory(replace(seed(3, "eg", GRAPH), created=now - timedelta(days=1)),
                        replace(seed(4, "eg"), created=now))
        api.readers.return_value = Readers.from_runs([run(101, "push")], api.run_id)
        self.assertEqual(sweep(api, POLICY, True)["deleted_bytes"], 0)
        api.readers.return_value = IDLE
        self.assertEqual(sweep(api, POLICY, True)["deleted_bytes"], 10)
        self.assertEqual(set(api.rows), {4})

    def test_pr_overlay_base_consumers_can_miss_a_superseded_database(self):
        prefix = "codeql-overlay-base-database-1-aaa-python-1.2.3-"
        baseline = [entry(1, prefix + "a"*40 + "-1-1"), entry(2, prefix + "b"*40 + "-2-1")]
        api = Inventory(*baseline)
        api.readers.return_value = Readers.from_runs([run(44, "pull_request", pulls=[7])], 100)
        self.assertEqual(sweep(api, POLICY, True)["deleted_bytes"], 10)
        self.assertIn(2, api.rows)
        api = Inventory(*baseline)
        api.readers.return_value = Readers.from_runs([run(45, "push")], 100)
        self.assertEqual(sweep(api, POLICY, True)["deleted_bytes"], 0)
        api.delete.assert_not_called()

    def test_reopened_pr_is_checked_at_deletion(self):
        api = Inventory(self.pr)
        api.closed_pull.side_effect = [True, False]
        self.assertEqual(sweep(api, POLICY, True)["held_candidate_bytes"], self.pr.size)
        api.delete.assert_not_called()

    def test_reader_inventory_fails_closed(self):
        api = Inventory(self.pr)
        api.readers.side_effect = RuntimeError("run inventory failed")
        with self.assertRaises(RuntimeError):
            sweep(api, POLICY, True)
        api.delete.assert_not_called()


class Contracts(unittest.TestCase):
    def test_all_pages_required(self):
        rows = [{"id": n} for n in range(101)]
        def fetch(page):
            return {"total_count": len(rows), "items": rows[(page-1)*100:page*100]}
        self.assertEqual(pages(fetch, "items"), rows)
        for bad in ({"total_count": 2, "items": []}, {"total_count": 2, "items": [{"id": 1}] * 2}):
            with self.assertRaises(ValueError):
                pages(lambda _: bad, "items")
        with self.assertRaises(ValueError):
            pages(lambda n: fetch(n) if n == 1 else {"total_count": 102, "items": []}, "items")

    def test_prerollover_defers_only_optional_upload_on_pressure_or_unknown_usage(self):
        with contextlib.redirect_stdout(io.StringIO()):
            for used, allowed in ((BUDGET - 1, "true"), (BUDGET, "false"), (BUDGET + 1, "false")):
                output = io.StringIO()
                api = Inventory(entry(1, "other", size=used))
                rollover(api, output)
                self.assertEqual(output.getvalue(), f"allowed={allowed}\n")
                api.delete.assert_not_called()
            output = io.StringIO()
            bad = Mock()
            bad.entries.side_effect = ValueError("incomplete inventory")
            with self.assertRaises(ValueError):
                rollover(bad, output)
            self.assertEqual(output.getvalue(), "")
        steps = yaml.safe_load((ROOT / ".github/workflows/staging-release.yml").read_text())["jobs"]["publish"]["steps"]
        ids = [s["name"] for s in steps if "name" in s]
        self.assertLess(ids.index("Build once, scan every layer, and publish exact bytes"),
                        ids.index("Check repository headroom before Cargo rollover"))
        save = next(s for s in steps if s.get("name") == "Retain accepted Cargo mounts")
        self.assertIn("steps.cargo-headroom.outcome == 'success'", save["if"])
        self.assertIn("steps.cargo-headroom.outputs.allowed == 'true'", save["if"])

    def test_concurrent_rust_writers_cannot_spend_the_same_headroom(self):
        # EG holds priority until its save completes. AM checks actual repository
        # bytes again after EG is visible, even when both started at 7.75 GiB.
        api = Inventory(entry(1, "pool", size=BUDGET - 2**28))
        api.eg_upload = Mock()
        def allowed(after_eg):
            output = io.StringIO()
            with contextlib.redirect_stdout(io.StringIO()):
                rollover(api, output, after_eg=after_eg, attempt=1, source="a"*40)
            return output.getvalue() == "allowed=true\n"
        self.assertTrue(allowed(False))
        for pending in ("pending", "unknown", "failure", "success"):
            api.eg_upload.return_value = pending
            self.assertFalse(allowed(True))
        peer = seed(99, "eg")
        api.rows[99] = replace(peer, size=2**29)
        self.assertFalse(allowed(True))
        api.rows[1] = replace(api.rows[1], size=2**30)
        self.assertTrue(allowed(True))
        api.rows[99] = replace(peer, ref="refs/pull/7/merge")
        self.assertFalse(allowed(True))
        api.eg_upload.return_value = "skipped"
        self.assertTrue(allowed(True))

    def test_overlapping_runs_defer_optional_saves_even_with_room(self):
        api = Inventory()
        api.sole_rust_writer.return_value = False
        output = io.StringIO()
        with contextlib.redirect_stdout(io.StringIO()):
            rollover(api, output, attempt=2, source="a"*40)
        self.assertEqual(output.getvalue(), "allowed=false\n")
        api.sole_rust_writer.assert_called_once_with(2, "a"*40)

    def test_rollover_requires_positive_evidence_of_only_its_exact_attempt(self):
        api = Actions("fixture", 99)
        own = run(99, "push") | {"head_sha": "a"*40, "run_attempt": 2, "path": ".github/workflows/staging-cd.yml"}
        for rows, allow in (([own], True), ([], False), ([own | {"run_attempt": 1}], False),
                            ([own | {"head_sha": "b"*40}], False), ([own | {"status": "queued"}], False),
                            ([own, own | {"id": 101}], False), ([own, own | {"id": 101, "status": "waiting"}], False)):
            api.list = lambda resource, field, **filters: [r for r in rows if r["status"] == filters["status"]]
            self.assertEqual(api.sole_rust_writer(2, "a"*40), allow)

    def test_eg_save_requires_the_exact_terminal_attempt_and_step(self):
        api = Actions("fixture", 99)
        step = {"name": "Retain accepted Cargo mounts", "status": "completed", "conclusion": "success"}
        job = {"name": "Release eg / Publish", "status": "completed", "run_id": 99,
               "head_sha": "a"*40, "run_attempt": 2, "steps": [step]}
        for change, expected in (({}, "success"), ({"status": "in_progress"}, "pending"),
                                 ({"head_sha": "b"*40}, "pending"), ({"steps": []}, "unknown"),
                                 ({"run_attempt": 1}, "pending"), ({"name": "other"}, "unknown")):
            api.list = Mock(return_value=[job | change])
            self.assertEqual(api.eg_upload(2, "a"*40), expected)
            api.list.assert_called_once_with("runs/99/attempts/2/jobs", "jobs")

    def test_develop_completion_authority_and_sweep_only_group(self):
        workflows = ROOT / ".github/workflows"
        load = lambda name: yaml.safe_load((workflows / (name + ".yml")).read_text())
        lock = load("cache-retention")["jobs"]["sweep"]["concurrency"]
        self.assertEqual(lock, {"group": "cma-develop-cache-sweep", "queue": "max", "cancel-in-progress": False})
        cd = load("staging-cd")
        self.assertNotIn("concurrency", cd)
        self.assertIn("['eg'] != ''", cd["jobs"]["release_am"]["with"]["eg_cache_writer"])
        for path in workflows.glob("*.y*ml"):
            content = path.read_text()
            self.assertNotIn("cma-develop-cache-readers", content, path.name)
            self.assertEqual(content.count(lock["group"]), int(path.stem == "cache-retention"), path.name)
        with TemporaryDirectory() as directory:
            path = Path(directory, "event.json")
            env = {"GITHUB_EVENT_NAME": "workflow_run", "GITHUB_REF": DEVELOP, "GITHUB_REPOSITORY": "ChronoAIProject/cma",
                   "GITHUB_WORKFLOW_REF": f"ChronoAIProject/cma/.github/workflows/cache-retention.yml@{DEVELOP}",
                   "GITHUB_EVENT_PATH": str(path)}
            event = {"status": "completed", "event": "push", "head_branch": "develop",
                     "head_repository": {"full_name": "ChronoAIProject/cma"}, "path": ".github/workflows/staging-cd.yml"}
            for altered in ({}, {"event": "pull_request"}, {"status": "in_progress"}, {"path": "other"},
                            {"head_repository": {"full_name": "other"}}, {"head_branch": "other"}):
                path.write_text(json.dumps({"action": "completed", "workflow_run": event | altered}))
                self.assertEqual(authorized_completion(env), not altered)

    def test_cache_inventory_rejects_invalid_rows(self):
        row = {"id": 1, "ref": DEVELOP, "key": "fixture", "version": "v", "size_in_bytes": 6,
               "created_at": CLOCK.isoformat(), "last_accessed_at": CLOCK.isoformat()}
        self.assertEqual(Entry.parse(row).size, 6)
        for bad in ({"id": True}, {"size_in_bytes": -1}, {"version": ""}, {"created_at": "2026-01-01"}):
            with self.assertRaises(ValueError):
                Entry.parse(row | bad)

    def test_rest_adapter_uses_exact_ids_and_all_nonterminal_run_states(self):
        api = Actions("fixture", 99)
        row = {"id": 1, "ref": DEVELOP, "key": "a#7", "version": "v", "size_in_bytes": 6,
               "created_at": CLOCK.isoformat(), "last_accessed_at": CLOCK.isoformat()}
        api.api.request = Mock(return_value={"total_count": 1, "actions_caches": [row]})
        self.assertEqual(api.visible(Entry.parse(row)), Entry.parse(row))
        self.assertIn("key=a%237", api.api.request.call_args.args[0])
        self.assertIsNone(api.visible(replace(Entry.parse(row), version="another")))
        api.api.request = Mock(return_value={"total_count": 0, "workflow_runs": []})
        self.assertEqual(api.readers(), IDLE)
        queries = [call.args[0] for call in api.api.request.call_args_list]
        for state in ("queued", "requested", "pending", "waiting", "in_progress"):
            self.assertTrue(any("status=" + state + "&" in path for path in queries))

    def test_public_pull_lookup_checks_identity_and_reads_fresh_state(self):
        ref = "refs/pull/7/merge"
        for state, repo, expected in (("closed", "ChronoAIProject/cma", True),
                                      ("open", "ChronoAIProject/cma", False),
                                      ("closed", "unrelated", None)):
            doc = {"number": 7, "state": state, "closed_at": CLOCK.isoformat(),
                   "base": {"repo": {"full_name": repo}}}
            api = Actions("fixture", 99)
            with patch("actions_cache_sweep.urlopen", side_effect=lambda *a, **kw: io.BytesIO(json.dumps(doc).encode())) as fetch:
                if expected is None:
                    with self.assertRaises(ValueError):
                        api.closed_pull(ref)
                else:
                    self.assertEqual(api.closed_pull(ref), expected)
                    self.assertEqual(api.closed_pull(ref), expected)
                self.assertEqual(fetch.call_count, 1 if expected is None else 2)

    def test_live_develop_ref_is_bound_to_exact_policy_revision(self):
        api = Actions("fixture", 99)
        revision = "3" * 40
        with patch("actions_cache_sweep.subprocess.run") as git:
            git.return_value.stdout = f"{revision}\t{DEVELOP}\n"
            api.require_live_policy(revision)
            git.return_value.stdout = f"{'4' * 40}\t{DEVELOP}\n"
            with self.assertRaisesRegex(ValueError, "develop changed"):
                api.require_live_policy(revision)
            with self.assertRaisesRegex(ValueError, "invalid retention"):
                api.require_live_policy(None)

    def test_dry_run_entrypoint_surfaces_nonzero_shortfall(self):
        api = Inventory(entry(1, "unknown", size=BUDGET + 1))
        with patch.object(sys, "argv", ["sweep"]), patch("actions_cache_sweep.Actions", return_value=api), \
                patch.dict(os.environ, {"GH_TOKEN": "fixture", "GITHUB_RUN_ID": "42"}, clear=True), \
                contextlib.redirect_stdout(io.StringIO()) as output:
            self.assertEqual(main(), 1)
            self.assertEqual(json.loads(output.getvalue())["over_budget_bytes"], 1)
            api.delete.assert_not_called()

    def test_permissions_triggers_and_apply_boundary(self):
        workflow = yaml.safe_load((ROOT / ".github/workflows/cache-retention.yml").read_text())
        self.assertEqual(set(workflow[True]), {"workflow_run", "workflow_dispatch"})
        trigger = workflow[True]["workflow_run"]
        self.assertEqual(trigger["types"], ["completed"])
        self.assertEqual(trigger["branches"], ["develop"])
        self.assertEqual(set(trigger["workflows"]), {"Staging CD", "Runtime Omarchy", "Migration harness"})
        self.assertEqual(workflow["permissions"], {})
        self.assertEqual(workflow["jobs"]["sweep"]["permissions"], {"actions": "write"})
        with patch.object(sys, "argv", ["sweep", "--apply"]), patch.dict(os.environ, {}, clear=True):
            with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                main()


if __name__ == "__main__":
    unittest.main()
