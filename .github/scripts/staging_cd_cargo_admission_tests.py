import contextlib
import io
import json
import os
import shutil
import subprocess
import sys
import unittest
from copy import deepcopy
from dataclasses import replace
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
from staging_cd_cache_retention_tests import ROOT, Inventory, entry, seed
from staging_cd_common import Refusal
from staging_cd_github import GitHub, GitHubNotFound

SHA = "3" * 40


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
        self.state = Account({})
        self.tip = None

    def read(self):
        return self.tip, deepcopy(self.state)

    def advance(self, previous, state, _source):
        if self.tip != previous:
            raise ValueError("competing budget ref")
        self.state = deepcopy(state)
        self.tip = (self.tip or 0) + 1


class Admission(unittest.TestCase):
    def setUp(self):
        self.ledger = Ledger()
        self.api = Inventory(entry(1, "unmanaged", size=BUDGET - GIB // 4))
        self.am, self.eg = seed(4, "am").key, seed(6, "eg").key

    def reserve(self, keys):
        return reserve(self.api, self.ledger, keys, SHA)

    def test_two_admissions_cannot_spend_the_same_last_quarter_gib(self):
        self.assertEqual(self.reserve([self.am])["admitted_bytes"], MAX_CHARGE)
        with patch("actions_cargo_admission.known_skipped", return_value=False):
            self.assertEqual(self.reserve([self.eg])["grants"], 0)
        self.assertEqual(self.ledger.state.charges, {self.am: MAX_CHARGE})
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

    def test_grant_committed_during_census_defers_the_older_writer(self):
        self.api.rows[1] = entry(1, "external", size=BUDGET - 3 * GIB // 2)
        self.assertEqual(self.reserve([self.am])["grants"], 1)
        am = authority() | {"CD_COMPONENT": "am"}
        eg = authority() | {"CD_COMPONENT": "eg", "GITHUB_RUN_ID": "6"}

        def concurrent_save():
            self.assertEqual(self.reserve([self.eg])["grants"], 1)
            self.assertTrue(save_decision(self.api, self.ledger, eg, ".", self.eg)["allowed"])
            # EG's row is still invisible when another producer's bytes appear.
            self.api.rows[2] = entry(2, "new-external", size=GIB)
            return self.api.entries()

        overlap = Mock()
        overlap.entries.side_effect = concurrent_save
        with (
            patch("actions_cargo_budget.bundle_bound", return_value=MIB),
            patch("actions_cargo_admission.known_skipped", return_value=False),
        ):
            changed = save_decision(overlap, self.ledger, am, ".", self.am)
            stable = save_decision(self.api, self.ledger, am, ".", self.am)
        self.assertEqual((changed["allowed"], changed["reason"]), (False, "budget-revision-changed"))
        self.assertEqual((stable["allowed"], stable["reason"]), (False, "repository-headroom"))
        self.assertEqual(changed["inventory_bytes"], BUDGET - GIB // 2)
        self.assertEqual(stable["inventory_bytes"] + stable["outstanding_bytes"], BUDGET - GIB // 2 + MAX_CHARGE)

    def test_positive_skip_releases_grant_and_both_services_fit_when_space_exists(self):
        self.api.rows[1] = replace(self.api.rows[1], size=GIB)
        self.ledger.state.charges[seed(9, "eg").key] = MAX_CHARGE
        with patch("actions_cargo_admission.known_skipped", return_value=True):
            result = self.reserve([self.eg, self.am])
        self.assertEqual((result["grants"], result["admitted_bytes"]), (2, 2 * MAX_CHARGE))
        self.assertEqual(set(self.ledger.state.charges), {self.eg, self.am})
        self.api.rows[10] = seed(10, "am")
        self.assertEqual(grant(Account({}), self.api.entries(), [self.am], Mock())["grants"], 0)

    def test_first_eligible_publication_can_save_both_v4_seeds_and_next_run_restores(self):
        import hashlib
        import io
        import tarfile
        from staging_cd_build_cache import BuildCache, bridge, prepare
        from staging_cd_publication_cache_tests import CacheTests
        from staging_cd_reservations import next_reservation
        from actions_cargo_budget import owner

        legacy = replace(seed(81, "am"), key=seed(81, "am").key.replace("-v4-", "-v3-"), size=GIB)
        self.api.rows = {1: entry(1, "other", size=5 * GIB), 81: legacy}
        with self.assertRaises(ValueError):
            owner(legacy.key)
        with TemporaryDirectory() as temp:
            directory = Path(temp)
            env = authority() | {"RUNNER_TEMP": temp}
            routes = {}
            for component in ("am", "eg"):
                reserved = next_reservation(component, SHA, "b" * 40, 10, "patch", 4, 1, [], [])
                routes[component] = json.dumps([{"component": component, "mode": "publish", "reservation": reserved}])
            with patch("actions_cargo_admission.Path.cwd", return_value=ROOT):
                keys = candidates(env | {"CARGO_PUBLICATIONS": json.dumps(routes)})
            self.assertIsNone(self.ledger.tip)
            result = self.reserve(keys)
            self.assertEqual((result["grants"], result["inventory_bytes"]), (2, 6 * GIB))
            for component, tasks in routes.items():
                service = env | {"CD_COMPONENT": component, "CD_TASKS": tasks}
                first = BuildCache(ROOT, service, ROOT, component, revision=SHA)
                key = first.key.archive_key(service)
                self.assertIn(key, self.ledger.state.charges)
                self.assertFalse(first.restore())
                tool = bridge()
                with (
                    patch("staging_cd_build_cache.bridge", return_value=tool),
                    patch.object(tool, "transfer") as transfer,
                ):
                    def populate_workspace(operation, target, **kwargs):
                        CacheTests().populate(target)
                        binary = {"am": "cma", "eg": "cmaeg"}[component]
                        output = f"release/deps/lib{binary}-0123456789abcdef.rlib"
                        data = b"fixture workspace artifact"
                        receipt = {
                            "compiler": {"binary": binary}, "files": {},
                            "outputs": {output: [0o644, hashlib.sha256(data).hexdigest()]},
                            "producer": {"CHRONO_BUILD_SHA": SHA},
                        }
                        archive = {output: data, ".cma-workspace-seed.json": json.dumps(receipt).encode()}
                        with tarfile.open(target / "target.tar.gz", "w:gz") as saved:
                            for path, content in archive.items():
                                member = tarfile.TarInfo(path)
                                member.size = len(content)
                                saved.addfile(member, io.BytesIO(content))

                    transfer.side_effect = populate_workspace
                    self.assertTrue(first.export_mounts())
                    self.assertTrue(save_decision(self.api, self.ledger, service, first.accepted, key)["allowed"])
                    stored = directory / key
                    shutil.copytree(first.accepted, stored)
                    self.assertEqual(
                        {p.name for p in stored.iterdir()}, {"manifest.json", "registry.tar.gz", "target.tar.gz"}
                    )
                    shutil.rmtree(first.accepted)
                    following = service | {"GITHUB_RUN_ID": "5"}
                    lookup = prepare(ROOT, following)
                    self.assertNotEqual(lookup["key"], key)
                    self.assertTrue(key.startswith(lookup["prefix"]))
                    self.assertFalse(legacy.key.startswith(lookup["family"]))
                    shutil.copytree(stored, first.accepted)
                    next_build = BuildCache(ROOT, following, ROOT, component, revision=SHA)
                    transfer.side_effect = None
                    self.assertTrue(next_build.restore())
                    self.assertEqual(next_build.restore_result, "hit")
                    transfer.assert_called_with("inject", next_build.accepted, namespace=next_build.namespace,
                                                timeout=360, max_content=6 * 1024 ** 3)
                    shutil.rmtree(first.accepted)

    def test_failed_inventory_and_invalid_receipt_cannot_write(self):
        for rows in (ValueError("truncated"), RuntimeError("unavailable")):
            self.api.entries = Mock(side_effect=rows)
            with self.assertRaises(type(rows)):
                self.reserve([self.am])
            self.assertIsNone(self.ledger.tip)
        state = Account({})
        state.charges[self.am] = MIB
        with self.assertRaisesRegex(Refusal, "exceeded"):
            grant(state, [replace(seed(4, "am"), size=MIB + 1)], [self.eg], Mock())
        for cap in (True, 0, MAX_CHARGE + 1):
            with self.assertRaises(ValueError):
                Account.decode(Account({}).encode() | {"charges": {self.am: cap}})
        with self.assertRaises(ValueError):
            Account.decode(Account({}).encode() | {"charges": {"unrelated": MIB}})

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


class Settlement(unittest.TestCase):
    def test_only_a_positive_skipped_step_or_job_releases_an_unseen_grant(self):
        api = Mock()
        run = {
            "id": 4,
            "run_attempt": 1,
            "status": "completed",
            "path": ".github/workflows/staging-cd.yml",
            "event": "push",
            "head_branch": "develop",
            "head_sha": SHA,
            "head_repository": {"full_name": "ChronoAIProject/cma"},
        }
        api.api.request.return_value = run
        job = {
            "name": "Release am / Publish",
            "run_id": 4,
            "run_attempt": 1,
            "head_sha": SHA,
            "status": "completed",
            "conclusion": "cancelled",
            "steps": [],
        }
        for jobs in ([], [job], [job, job], [job | {"name": "other"}]):
            api.list.return_value = jobs
            self.assertFalse(known_skipped(api, (4, 1, "am")))
        for result in ("success", "failure", "cancelled", "skipped"):
            step = {"name": "Retain accepted Cargo mounts", "status": "completed", "conclusion": result}
            api.list.return_value = [job | {"steps": [step]}]
            self.assertEqual(known_skipped(api, (4, 1, "am")), result == "skipped")
        api.list.return_value = [job | {"conclusion": "skipped"}]
        self.assertTrue(known_skipped(api, (4, 1, "am")))
        api.api.request.return_value = run | {"status": "pending"}
        self.assertFalse(known_skipped(api, (4, 1, "am")))
        with self.assertRaises(ValueError):
            known_skipped(api, (4, 2, "am"))


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
        # The release workflows no longer call the cache budget; the entrypoint
        # itself must still refuse any caller outside the develop release.
        cd = yaml.safe_load((ROOT / ".github/workflows/staging-cd.yml").read_text())["jobs"]
        self.assertNotIn("cargo_budget", cd)
        publish_job = yaml.safe_load((ROOT / ".github/workflows/staging-release.yml").read_text())["jobs"]["publish"]
        self.assertEqual(publish_job["permissions"]["contents"], "read")
        self.assertFalse(any(s.get("id") == "cargo-headroom" for s in publish_job["steps"]))
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
        api = Inventory()
        api.entries = Mock(side_effect=ValueError("incomplete"))
        with self.assertRaisesRegex(ValueError, "incomplete"):
            reserve(api, ledger, [], SHA)
        self.assertIsNone(ledger.tip)
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
                    contextlib.redirect_stdout(io.StringIO()) as log,
                ):
                    adapter.return_value.entries.return_value = []
                    ref.return_value.read.return_value = (None, Account({}))
                    main()
                doc = json.loads(log.getvalue())
                if mode == "reserve":
                    self.assertEqual(doc["grants"], 0)
                else:
                    self.assertFalse(doc["allowed"])
                    self.assertEqual(output.read_text(), "allowed=false\n")
