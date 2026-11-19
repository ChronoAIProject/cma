import contextlib
import io
import json
import os
import subprocess
import sys
import unittest
from copy import deepcopy
from dataclasses import replace
from datetime import datetime, timedelta, timezone
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import Mock, patch

import yaml

from actions_cache_retention import BUDGET, DEVELOP
from actions_cache_sweep import Actions
from actions_cargo_admission import candidates, known_skipped, main, reserve
from actions_cargo_budget import (
    Account,
    BudgetRef,
    GIB,
    MAX_CHARGE,
    MIB,
    REF,
    bundle_bound,
    grant,
    save_decision,
    stored_bound,
)
from actions_cargo_migration import FIRST_WRITER, SAVE, WriterHistory, has_upload, legacy_charge, originals
from staging_cd_cache_retention_tests import ROOT, Inventory, entry, seed
from staging_cd_common import Refusal
from staging_cd_github import GitHub, GitHubNotFound

SHA = "3" * 40
NOW = datetime(2026, 1, 1, tzinfo=timezone.utc)


def account(done=True):
    return Account({}, {"complete": done, "seen": [], "unsettled": [], "debt": 0 if done else None})


def release(number, era="legacy", attempt=1):
    source = {"before": "1", "legacy": "2", "fenced": "3", "coordinated": "4"}[era] * 40
    return {
        "id": number,
        "run_number": number,
        "run_attempt": attempt,
        "head_sha": source,
        "event": "push",
        "path": ".github/workflows/staging-cd.yml",
        "status": "completed",
        "head_branch": "develop",
        "head_repository": {"full_name": "ChronoAIProject/cma"},
        "created_at": (NOW - timedelta(days=31)).isoformat(),
    }


def publish(run, attempt, service, conclusion):
    return {
        "run_id": run["id"],
        "run_attempt": attempt,
        "head_sha": run["head_sha"],
        "name": f"Release {service} / Publish",
        "conclusion": conclusion,
        "status": "completed",
        "steps": [{"name": SAVE, "status": "completed", "conclusion": conclusion}],
    }


def authority():
    return {
        "GITHUB_REPOSITORY": "ChronoAIProject/cma",
        "GITHUB_REF": DEVELOP,
        "GITHUB_EVENT_NAME": "push",
        "GITHUB_WORKFLOW_REF": "ChronoAIProject/cma/.github/workflows/staging-cd.yml@" + DEVELOP,
        "GITHUB_RUN_ID": "4",
        "GITHUB_RUN_ATTEMPT": "1",
        "GITHUB_SHA": SHA,
        "GH_TOKEN": "fixture",
    }


class Ledger:
    def __init__(self):
        self.state = account()
        self.tip = 1

    def read(self):
        return self.tip, deepcopy(self.state)

    def advance(self, previous, state, _source):
        if self.tip != previous:
            raise ValueError("competing budget ref")
        self.state = deepcopy(state)
        self.tip += 1


class Admission(unittest.TestCase):
    def setUp(self):
        self.ledger = Ledger()
        self.api = Inventory(entry(1, "unmanaged", size=7 * GIB + 3 * GIB // 4))
        self.am, self.eg = seed(4, "am").key, seed(6, "eg").key

    def reserve(self, keys):
        return reserve(self.api, self.ledger, keys, SHA, None, NOW)

    def test_two_admissions_cannot_spend_the_same_last_quarter_gib(self):
        self.assertEqual(self.reserve([self.am])["admitted_bytes"], GIB // 4)
        with patch("actions_cargo_admission.known_skipped", return_value=False):
            self.assertEqual(self.reserve([self.eg])["grants"], 0)
        self.assertEqual(self.ledger.state.charges, {self.am: GIB // 4})
        ledger = self.ledger
        first, a = ledger.read()
        other, b = ledger.read()
        ledger.advance(first, a, SHA)
        with self.assertRaisesRegex(ValueError, "competing"):
            ledger.advance(other, b, SHA)

    def test_cancelled_half_gib_save_remains_charged_until_visible(self):
        self.ledger.state.charges[self.am] = GIB // 2
        with patch("actions_cargo_admission.known_skipped", return_value=False):
            result = self.reserve([self.eg])
        self.assertEqual((result["grants"], result["outstanding_bytes"]), (0, GIB // 2))
        self.api.rows[8] = replace(seed(4, "am"), size=GIB // 2)
        result = self.reserve([self.eg])
        self.assertEqual(
            (result["grants"], result["outstanding_bytes"], result["inventory_bytes"]), (0, 0, BUDGET + GIB // 4)
        )
        self.assertEqual(self.ledger.state.charges, {})

    def test_positive_skip_releases_grant_and_both_services_fit_when_space_exists(self):
        self.api.rows[1] = replace(self.api.rows[1], size=GIB)
        self.ledger.state.charges[seed(9, "eg").key] = MAX_CHARGE
        with patch("actions_cargo_admission.known_skipped", return_value=True):
            result = self.reserve([self.eg, self.am])
        self.assertEqual((result["grants"], result["admitted_bytes"]), (2, 2 * MAX_CHARGE))
        self.assertEqual(set(self.ledger.state.charges), {self.eg, self.am})
        self.api.rows[10] = seed(10, "am")
        self.assertEqual(grant(account(), self.api.entries(), [self.am], Mock(), 0)["grants"], 0)

    def test_failed_inventory_and_invalid_receipt_cannot_write(self):
        for rows in (ValueError("truncated"), RuntimeError("unavailable")):
            self.api.entries = Mock(side_effect=rows)
            with self.assertRaises(type(rows)):
                self.reserve([self.am])
            self.assertEqual(self.ledger.tip, 1)
        state = account()
        state.charges[self.am] = MIB
        with self.assertRaisesRegex(Refusal, "exceeded"):
            grant(state, [replace(seed(4, "am"), size=MIB + 1)], [self.eg], Mock(), 0)
        for cap in (True, 0, MAX_CHARGE + 1):
            with self.assertRaises(ValueError):
                Account.decode(account().encode() | {"charges": {self.am: cap}})
        with self.assertRaises(ValueError):
            Account.decode(account().encode() | {"charges": {"unrelated": MIB}})

    def test_bundle_cap_owner_manifest_and_exact_boundary(self):
        self.reserve([self.am])
        env = authority() | {"CD_COMPONENT": "am"}
        with TemporaryDirectory() as temp:
            path = Path(temp)
            (path / "manifest.json").write_text(json.dumps({"prefix": self.am.rsplit("-", 2)[0] + "-"}))
            for name in ("target", "registry"):
                (path / (name + ".tar.gz")).write_bytes(b"fixture")
            self.assertTrue(save_decision(self.api, self.ledger, env, path, self.am)["allowed"])
            for changed in ({"GITHUB_RUN_ID": "5"}, {"GITHUB_RUN_ATTEMPT": "2"}, {"CD_COMPONENT": "eg"}):
                with self.assertRaisesRegex(Refusal, "owner"):
                    save_decision(self.api, self.ledger, env | changed, path, self.am)
            self.api.rows[1] = replace(self.api.rows[1], size=BUDGET)
            self.assertFalse(save_decision(self.api, self.ledger, env, path, self.am)["allowed"])
            self.api.rows[1] = replace(self.api.rows[1], size=1)
            self.ledger.state.charges[self.am] = MIB
            self.assertFalse(save_decision(self.api, self.ledger, env, path, self.am)["allowed"])
            (path / "manifest.json").write_text('{"prefix":"different"}')
            with self.assertRaisesRegex(Refusal, "manifest"):
                bundle_bound(path, self.am)
            (path / "unexpected").touch()
            with self.assertRaisesRegex(Refusal, "directory"):
                bundle_bound(path, self.am)
        self.assertGreater(MAX_CHARGE, GIB + 4096)
        self.assertLessEqual(stored_bound(GIB + 4095), MAX_CHARGE)


class Migration(unittest.TestCase):
    def setUp(self):
        self.runs = [release(4, "coordinated"), release(3, "fenced"), release(2), release(1, "before")]
        self.history = Mock()
        self.history.era.side_effect = lambda sha: {"1": "before", "2": "legacy", "3": "fenced", "4": "coordinated"}[
            sha[0]
        ]
        self.api = Mock(run_id=4)
        self.jobs = {2: [publish(self.runs[2], 1, "am", "skipped")], 3: [publish(self.runs[1], 1, "eg", "skipped")]}
        self.api.list.side_effect = lambda path, field: (
            self.runs if field == "workflow_runs" else self.jobs[int(path.split("/")[1])]
        )
        self.state = account(False).migration

    def charge(self, rows=()):
        return legacy_charge(self.api, self.history, self.state, rows, NOW)

    def test_original_eligibility_window_can_outlast_a_skipped_attempt(self):
        # A #2473 allocator rerun can create fresh AM and EG reservations.
        self.runs[2]["created_at"] = (NOW - timedelta(days=29)).isoformat()
        self.assertIsNone(self.charge())
        self.runs[2]["created_at"] = (NOW - timedelta(days=30)).isoformat()
        self.assertIsNone(self.charge())
        self.runs[2]["created_at"] = (NOW - timedelta(days=31)).isoformat()
        self.runs[2]["status"] = "queued"
        self.assertIsNone(self.charge())
        self.runs[2]["status"] = "completed"
        # The audited tree cannot start a future stale-tip save, even today.
        self.runs[1]["created_at"] = NOW.isoformat()
        self.assertEqual(self.charge(), 0)
        self.assertTrue(self.state["complete"])

    def test_legacy_inflight_and_acceptance_on_cancellation_are_separate(self):
        self.runs[1]["status"] = "in_progress"
        self.assertIsNone(self.charge())
        self.runs[1]["status"] = "completed"
        self.jobs[3] = [publish(self.runs[1], 1, "eg", "cancelled")]
        self.assertEqual(self.charge(), MAX_CHARGE)
        pr = replace(seed(3, "eg"), ref="refs/pull/3/merge")
        self.assertEqual(self.charge([pr]), MAX_CHARGE)
        self.assertEqual(self.charge([seed(3, "eg")]), 0)
        self.assertEqual(self.state["seen"], [])
        self.api.list.reset_mock()
        self.assertEqual(self.charge(), 0)
        self.api.list.assert_not_called()

    def test_only_positive_inventory_or_explicit_nonupload_settles_all_attempts(self):
        self.runs[2]["run_attempt"] = 2
        self.jobs[2] = [publish(self.runs[2], 1, "am", "cancelled")]
        with self.assertRaisesRegex(Refusal, "job inventory"):
            self.charge()
        attempt2 = publish(self.runs[2], 2, "am", "success")
        self.api.list.side_effect = lambda path, field: (
            self.runs
            if field == "workflow_runs"
            else [attempt2]
            if "/attempts/2/" in path
            else self.jobs[int(path.split("/")[1])]
        )
        self.runs[2]["created_at"] = NOW.isoformat()
        self.assertIsNone(self.charge([seed(2, "am")]))
        self.runs[2]["created_at"] = (NOW - timedelta(days=31)).isoformat()
        self.assertEqual(self.charge(), MAX_CHARGE)
        self.assertTrue(self.state["complete"])
        self.api.list.reset_mock()
        self.assertEqual(self.charge(), MAX_CHARGE)
        self.api.list.assert_not_called()
        replacement = replace(seed(2, "am"), key=seed(2, "am").key[:-1] + "2")
        self.assertEqual(self.charge([replacement]), 0)

    def test_incomplete_originals_and_job_history_fail_closed(self):
        for rows in (
            self.runs[:2] + self.runs[3:],
            self.runs[:-1],
            self.runs[1:],
            [self.runs[0] | {"event": "pull_request"}],
        ):
            self.api.list.side_effect = None
            self.api.list.return_value = rows
            with self.assertRaises(ValueError):
                originals(self.api, self.history)
        run = self.runs[2]
        job = publish(run, 1, "am", "success")
        for conclusion in ("cancelled", "failure", "success"):
            self.assertTrue(has_upload([job | {"steps": [], "conclusion": conclusion}], run, 1, "am"))
        self.assertFalse(has_upload([job | {"steps": [], "conclusion": "skipped"}], run, 1, "am"))
        with self.assertRaises(ValueError):
            has_upload([job, job], run, 1, "am")

    def test_exact_attempt_is_required_for_a_positive_skip(self):
        self.api.api.request.return_value = self.runs[2] | {"status": "pending"}
        self.assertFalse(known_skipped(self.api, (2, 1, "am")))
        self.api.api.request.return_value = self.runs[2]
        self.assertTrue(known_skipped(self.api, (2, 1, "am")))
        for step in ("failure", "cancelled", "success"):
            self.jobs[2] = [publish(self.runs[2], 1, "am", step)]
            self.assertFalse(known_skipped(self.api, (2, 1, "am")))
        with self.assertRaises(ValueError):
            known_skipped(self.api, (2, 2, "am"))

    def test_checked_automation_tree_and_cutover_order(self):
        head = "f" * 40
        guarded = "3" * 40
        earlier = "1" * 40

        def revision(args, **kwargs):
            if args[1] == "log":
                return Mock(stdout=head)
            if args[1] == "rev-list":
                return Mock(stdout="\n".join([earlier, FIRST_WRITER, guarded, head]))
            return Mock(
                stdout=("29e024efdf89b68d37e25babef13679df384f07f" if args[-1] == guarded + ":.github" else "6" * 40)
            )

        with patch("actions_cargo_migration.subprocess.run", side_effect=revision):
            history = WriterHistory(head)
            self.assertEqual(history.era(earlier), "before")
            self.assertEqual(history.era(FIRST_WRITER), "legacy")
            self.assertEqual(history.era(guarded), "fenced")
            self.assertEqual(history.era(head), "coordinated")


class Contracts(unittest.TestCase):
    def test_atomic_fast_forward_rejects_two_siblings_and_ambiguous_writes(self):
        # Exercise the production ref adapter against Git's ancestry rule.
        with TemporaryDirectory() as temp:

            def git(*args, data=None):
                return subprocess.check_output(
                    ["git", *args],
                    cwd=temp,
                    text=True,
                    input=data,
                    env=os.environ
                    | {
                        "GIT_AUTHOR_NAME": "fixture",
                        "GIT_COMMITTER_NAME": "fixture",
                        "GIT_AUTHOR_EMAIL": "fixture@example.invalid",
                        "GIT_COMMITTER_EMAIL": "fixture@example.invalid",
                    },
                ).strip()

            git("init", "--bare", "--quiet")
            tree = git("mktree", data="")
            source = git("commit-tree", tree, data="original")
            state = {}

            def request(path, payload=None, **kwargs):
                if path == "git/ref/" + REF:
                    if REF not in state:
                        raise GitHubNotFound("absent")
                    return {"ref": "refs/" + REF, "object": {"type": "commit", "sha": state[REF]}}
                if path.startswith("git/commits/"):
                    sha = path.rsplit("/", 1)[1]
                    return {"sha": sha, "tree": {"sha": tree}, "message": git("show", "-s", "--format=%B", sha)}
                if path == "git/commits":
                    return {"sha": git("commit-tree", tree, "-p", payload["parents"][0], data=payload["message"])}
                if path == "git/refs/" + REF:
                    self.assertEqual((kwargs["method"], payload["force"]), ("PATCH", False))
                    if git("merge-base", state[REF], payload["sha"]) != state[REF]:
                        raise ValueError("non-fast-forward")
                elif path == "git/refs":
                    self.assertEqual(payload["ref"], "refs/" + REF)
                    if REF in state:
                        raise ValueError("exists")
                else:
                    self.fail("unexpected budget endpoint")
                state[REF] = payload["sha"]
                return {"ref": "refs/" + REF, "object": {"sha": state[REF]}}

            api = Mock()
            api.request.side_effect = request
            store = BudgetRef(api)
            base, a = store.read()
            self.assertIsNone(base)
            a.charges[seed(1, "am").key] = MIB
            store.advance(base, a, source)
            with self.assertRaisesRegex(ValueError, "exists"):
                store.advance(base, a, source)
            tip, first = store.read()
            _, second = store.read()
            first.charges[seed(2, "eg").key] = GIB // 2
            second.charges[seed(3, "am").key] = GIB // 2
            store.advance(tip, first, source)
            with self.assertRaisesRegex(ValueError, "non-fast-forward"):
                store.advance(tip, second, source)
            self.assertEqual(store.read()[1].charges, first.charges)

    def test_transport_uses_patch_without_force_and_rejects_unknown_methods(self):
        runner = Mock(return_value=Mock(stdout="{}", returncode=0))
        api = GitHub("fixture", runner=runner)
        api.request("git/refs/" + REF, {"force": False, "sha": SHA}, method="PATCH")
        args = runner.call_args.args[0]
        self.assertEqual(args[args.index("--method") + 1], "PATCH")
        with self.assertRaises(ValueError):
            api.request("git/refs/" + REF, {}, method="DELETE")
        runner.assert_called_once()

    def test_job_permissions_failure_isolation_and_authority(self):
        cd = yaml.safe_load((ROOT / ".github/workflows/staging-cd.yml").read_text())["jobs"]
        budget = cd["cargo_budget"]
        self.assertTrue(budget["continue-on-error"])
        self.assertEqual(budget["permissions"], {"actions": "read", "contents": "write"})
        self.assertFalse(any("cargo_budget" in j.get("needs", []) for j in cd.values()))
        self.assertEqual(budget["needs"], "allocate")
        publish_job = yaml.safe_load((ROOT / ".github/workflows/staging-release.yml").read_text())["jobs"]["publish"]
        self.assertEqual(publish_job["permissions"]["contents"], "read")
        gate = next(s for s in publish_job["steps"] if s.get("id") == "cargo-headroom")
        self.assertTrue(gate["continue-on-error"])
        self.assertIn("publication.outputs.build-cache-key", gate["env"]["CARGO_SAVE_KEY"])
        for altered in (
            {"GITHUB_REPOSITORY": "other"},
            {"GITHUB_EVENT_NAME": "pull_request"},
            {"GITHUB_REF": "refs/heads/other"},
            {"GITHUB_WORKFLOW_REF": "other"},
        ):
            with (
                patch.dict(os.environ, authority() | altered, clear=True),
                patch.object(sys, "argv", ["budget", "reserve"]),
                patch("actions_cargo_admission.Actions") as adapter,
                self.assertRaises(ValueError),
            ):
                main()
            adapter.assert_not_called()

    def test_only_a_publish_reservation_owned_by_this_attempt_can_request_bytes(self):
        current = {"run_id": 4, "run_attempt": 1}
        task = {"component": "am", "mode": "publish", "reservation": current}
        env = authority()

        def routed(task):
            return env | {
                "CARGO_PUBLICATIONS": json.dumps({"eg": json.dumps([{"mode": "resume"}]), "am": json.dumps([task])})
            }

        with patch("actions_cargo_admission.lookup_key") as lookup:
            lookup.return_value.archive_key.return_value = seed(4, "am").key
            self.assertEqual(candidates(routed(task)), [seed(4, "am").key])
            with self.assertRaises(ValueError):
                candidates(routed(task | {"reservation": current | {"run_attempt": 2}}))
            lookup.assert_called_once()

    def test_incomplete_history_cannot_mutate_budget_and_date_range_survives_transport(self):
        ledger = Ledger()
        ledger.state = account(False)
        with patch("actions_cargo_admission.legacy_charge", side_effect=ValueError("incomplete")):
            with self.assertRaisesRegex(ValueError, "incomplete"):
                reserve(Inventory(), ledger, [], SHA, None, NOW)
        self.assertEqual(ledger.tip, 1)
        platform = Actions("fixture", 4)
        runner = Mock(return_value=Mock(stdout='{"total_count":0,"workflow_runs":[]}', returncode=0))
        platform.api.runner = runner
        self.assertEqual(platform.list("workflows/staging-cd.yml/runs", "workflow_runs", created="start..end"), [])
        self.assertIn("created=start%2E%2Eend", " ".join(runner.call_args.args[0]))

    def test_public_entrypoints_use_published_grant_and_skip_on_missing_identity(self):
        with TemporaryDirectory() as temp:
            output = Path(temp, "output")
            env = authority() | {
                "RUNNER_TEMP": temp,
                "GITHUB_OUTPUT": str(output),
                "CD_COMPONENT": "am",
                "CARGO_SAVE_KEY": seed(4, "am").key,
                "CARGO_PUBLICATIONS": "{}",
            }
            for mode in ("reserve", "check"):
                with (
                    patch.dict(os.environ, env, clear=True),
                    patch.object(sys, "argv", ["budget", mode]),
                    patch("actions_cargo_admission.Actions") as adapter,
                    patch("actions_cargo_admission.BudgetRef") as ref,
                    patch("actions_cargo_admission.WriterHistory"),
                    contextlib.redirect_stdout(io.StringIO()) as log,
                ):
                    adapter.return_value.entries.return_value = []
                    ref.return_value.read.return_value = (None, account())
                    main()
                doc = json.loads(log.getvalue())
                if mode == "reserve":
                    self.assertEqual(doc["grants"], 0)
                else:
                    self.assertFalse(doc["allowed"])
                    self.assertEqual(output.read_text(), "allowed=false\n")
