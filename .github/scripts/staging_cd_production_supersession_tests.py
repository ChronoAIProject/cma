"""Production supersession is measured against main's newest release, never develop's head."""
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

import prod_cd_select as select
import staging_cd_invoke as invoke
import staging_cd_invoke_tests as invoke_fixtures
from staging_cd_common import Refusal
from staging_cd_deploy_result_tests import fixture_result
from staging_cd_history import DevelopHistory
from staging_cd_overtaken_deploy_tests import receipt
from staging_cd_receipts import canonical
from staging_cd_set import release_set
from staging_cd_versions import DEPLOYABLES
from staging_cd_watermark import SupersededVersions, require_released_versions

# Develop first parents, oldest first: every receipt's source, an ancestor of
# the release R, R itself, then develop commits a later release may merge.
OLDER, BETWEEN, RELEASED, AFTER, NEWEST = "a" * 40, "b" * 40, "2" * 40, "e" * 40, "f" * 40
CHAIN = [NEWEST, AFTER, RELEASED, BETWEEN, OLDER]
# Main first parents: this run's merge M of R, and a later release merge of NEWEST.
PREVIOUS, MERGE, NEWER_MERGE, UNRELATED = "4" * 40, "1" * 40, "3" * 40, "c" * 40
MAIN = [NEWER_MERGE, MERGE, PREVIOUS]
PARENTS = {MERGE: [PREVIOUS, RELEASED], NEWER_MERGE: [MERGE, NEWEST]}
ALL = ["am", "eg", "runtime", "runtime-omarchy", "web"]


def history(newest=RELEASED):
    """Develop's first parents from the newest release's develop commit."""
    return DevelopHistory(newest, CHAIN[CHAIN.index(newest):])


def advertisement(*finals):
    """Each receipt's own final at OLDER plus `finals` as (component, version, source)."""
    rows = [(key, "0.10.1", OLDER) for key in ALL] + list(finals)

    def advertise(prefixes, *, runner, peeled=False):
        lines = []
        for index, (key, version, source) in enumerate(rows):
            ref = f"refs/tags/{DEPLOYABLES[key]}/v{version}"
            if any(ref.startswith(prefix) for prefix in prefixes):
                lines.append(f"{index + 1:040x}\t{ref}")
                if peeled:
                    lines.append(f"{source}\t{ref}^{{}}")
        return "\n".join(lines)
    return advertise


class ReleasedVersionsTests(unittest.TestCase):
    def check(self, *finals, keys=("web",), newest=RELEASED, version="0.10.1"):
        receipts = {key: receipt(key, version=version) for key in keys}
        with patch("staging_cd_watermark.advertise", side_effect=advertisement(*finals)):
            require_released_versions(receipts, history(newest))

    def test_only_a_higher_final_on_the_release_history_supersedes(self):
        # Develop publishes past every release; that final is a later release's to deploy.
        self.check(("web", "0.10.2", AFTER), ("eg", "0.10.2", NEWEST), keys=("eg", "web"))
        for source in (BETWEEN, RELEASED):
            with self.subTest(source=source), self.assertRaises(SupersededVersions) as raised:
                self.check(("web", "0.10.2", source), ("web", "0.10.3", AFTER), ("eg", "0.10.2", AFTER),
                           keys=("eg", "web"))
            self.assertEqual(raised.exception.components, {"web"})
        # The bound is the newest release's history, so it moves with each release.
        with self.assertRaises(SupersededVersions):
            self.check(("web", "0.10.2", AFTER), newest=NEWEST)

    def test_each_receipt_tag_must_still_be_advertised(self):
        with self.assertRaisesRegex(Refusal, "absent from component inventory"):
            self.check(version="0.10.2")


def git_runner(parents):
    """release_commit's local reads: a commit's parents, and one tree shared by M and R."""
    def run(args, **_kwargs):
        if args[3:5] == ["rev-list", "--parents"]:
            return subprocess.CompletedProcess(args, 0, " ".join([args[-1], *parents.get(args[-1], [])]) + "\n", "")
        if args[3] == "rev-parse":
            return subprocess.CompletedProcess(args, 0, "t" * 40 + "\n", "")
        return subprocess.CompletedProcess(args, 1, "", "")
    return run


class ReleaseBoundsTests(unittest.TestCase):
    def bounds(self, head, *, parents=PARENTS, main=MAIN):
        def from_head(commit, *, root):
            self.assertEqual(root, Path("."))
            if commit in main:
                return DevelopHistory(commit, main[main.index(commit):])
            return DevelopHistory(commit, [UNRELATED, OLDER]) if commit == UNRELATED else history(commit)

        with patch.object(select.DevelopHistory, "from_head", side_effect=from_head) as reads:
            bounds = select.release_bounds({"GITHUB_SHA": MERGE}, Path("."), head=lambda: head,
                                           runner=git_runner(parents))
        # Main's head is fetched and proved first; then the newest release's develop history.
        self.assertEqual([call.args[0] for call in reads.call_args_list], [head, bounds[1].head])
        return bounds

    def test_main_head_release_bounds_this_runs_release(self):
        released, bound = self.bounds(MERGE)
        self.assertEqual((released, bound.head), (RELEASED, RELEASED))
        # A later release merged while this one waited: its develop commit is the bound.
        released, bound = self.bounds(NEWER_MERGE)
        self.assertEqual((released, bound.head), (RELEASED, NEWEST))
        self.assertIn(RELEASED, bound.positions)

    def test_a_main_head_that_is_not_a_later_release_is_refused(self):
        with self.assertRaisesRegex(Refusal, "main's first-parent history"):
            self.bounds(NEWER_MERGE, main=[NEWER_MERGE, PREVIOUS])
        with self.assertRaisesRegex(Refusal, "merge commit"):
            self.bounds(NEWER_MERGE, parents=dict(PARENTS, **{NEWER_MERGE: [MERGE]}))
        with self.assertRaisesRegex(Refusal, "newest release's develop history"):
            self.bounds(NEWER_MERGE, parents=dict(PARENTS, **{NEWER_MERGE: [MERGE, UNRELATED]}))


class MainHeadTests(unittest.TestCase):
    def test_main_head_is_read_from_one_credential_free_advertisement(self):
        runner = Mock(return_value=subprocess.CompletedProcess([], 0, MERGE + "\trefs/heads/main\n", ""))
        with patch.dict(os.environ, {"GH_TOKEN": "unrelated", "PATH": os.environ["PATH"]}, clear=True):
            self.assertEqual(select.main_head(runner=runner), MERGE)
        args, env = runner.call_args.args[0], runner.call_args.kwargs["env"]
        self.assertEqual(args[-2:], ["https://github.com/ChronoAIProject/cma.git", "refs/heads/main"])
        self.assertIn("credential.helper=", args)
        self.assertNotIn("GH_TOKEN", env)
        for stdout, code in (("", 0), (MERGE + "\trefs/heads/main-next\n", 0), ("main\trefs/heads/main\n", 0),
                             (MERGE + "\trefs/heads/main\n" + NEWER_MERGE + "\trefs/heads/main\n", 0),
                             (MERGE + "\trefs/heads/main\n", 128)):
            with self.subTest(stdout=stdout, code=code), self.assertRaisesRegex(Refusal, "Main head"):
                select.main_head(runner=Mock(return_value=subprocess.CompletedProcess([], code, stdout, "")))
        with self.assertRaisesRegex(Refusal, "Main head"):
            select.main_head(runner=Mock(side_effect=subprocess.TimeoutExpired("git", 30)))


class ProductionDeployTests(unittest.TestCase):
    """Under the production set lock, with every receipt at OLDER and this run releasing R."""

    def deploy(self, keys, *finals, newest=RELEASED, target="production", policy=None, event="push"):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            env = invoke_fixtures.InvokeTests().environment(root)
            env["CD_RELEASE_SET"] = canonical(release_set({key: receipt(key) for key in keys}))
            if target is not None:
                payload = ({"after": MERGE, "deleted": False} if event == "push" else {"inputs": {"infra_ref": ""}})
                Path(env["GITHUB_EVENT_PATH"]).write_text(json.dumps(
                    {"ref": "refs/heads/main", "repository": {"full_name": "ChronoAIProject/cma"}, **payload}))
                env.update(CD_TARGET=target, GITHUB_REF="refs/heads/main", GITHUB_SHA=MERGE, GITHUB_EVENT_NAME=event)
            deployed, named, reads = [], [], []

            def controller(_script, path, *_args):
                deployed.append(sorted(json.loads(path.read_text())["components"]))
                fixture_result(path)
                return 0

            def from_head(head):
                reads.append(head)
                return history(head)

            def run_selected(runner, script, path, _request, _receipts, started, mode,
                             _target, _env, child_env, _limit, _masker):
                return runner(script, path, mode, started, child_env)

            bounds = Mock(return_value=(RELEASED, history(newest)))
            with patch("staging_cd_watermark.advertise", side_effect=advertisement(*finals)), \
                    patch.object(invoke, "release_bounds", bounds), \
                    patch.object(invoke.DevelopHistory, "from_head", side_effect=from_head), \
                    patch.object(invoke, "run_release", side_effect=run_selected), \
                    patch.object(invoke, "ContractPolicy", policy or Mock(side_effect=AssertionError)):
                try:
                    code = invoke.deploy(env, Mock(), lambda _env: root / "deploy_set.py", controller,
                                         policy_reader=lambda _script, selected, **_kw: sorted(selected),
                                         on_overtaken=named.extend)
                except invoke.NotAdmitted as refusal:
                    code = refusal.reason
            return code, deployed, named, bounds, reads

    def test_a_release_deploys_although_develop_published_newer_finals_after_it(self):
        code, deployed, named, bounds, reads = self.deploy(ALL, *((key, "0.10.2", AFTER) for key in ALL))
        self.assertEqual((code, deployed, named), (0, [ALL], []))
        self.assertEqual(bounds.call_args.args[0]["GITHUB_SHA"], MERGE)
        # Neither develop's head nor main's own first parents bound production.
        self.assertEqual(reads, [])
        # A final after the newest release is the next release's, whichever release that is.
        code, deployed, *_ = self.deploy(["web"], ("web", "0.10.2", NEWEST), newest=AFTER)
        self.assertEqual((code, deployed), (0, [["web"]]))

    def test_a_newer_final_at_or_before_the_release_supersedes_its_receipt(self):
        for source in (BETWEEN, RELEASED):
            with self.subTest(source=source):
                code, deployed, named, *_ = self.deploy(
                    ["eg", "web"], ("web", "0.10.2", source), ("web", "0.10.3", AFTER))
                self.assertEqual((code, deployed, named), (0, [["eg"]], []))
                # No newer release owns that final, so a wholly superseded set fails, never skips.
                code, deployed, named, *_ = self.deploy(["web"], ("web", "0.10.2", source))
                self.assertEqual((code, deployed, named), ("authority", [], []))

    def test_a_later_release_that_overtook_the_set_takes_over_instead_of_a_rollback(self):
        code, deployed, named, *_ = self.deploy(ALL, *((key, "0.10.2", AFTER) for key in ALL), newest=NEWEST)
        self.assertEqual((code, deployed, named), (invoke.SUPERSEDED_BY_NEWER_FINAL, [], ALL))
        # An unchanged member is the very version the newer release deploys; the older web is withheld.
        code, deployed, named, *_ = self.deploy(["eg", "web"], ("web", "0.10.2", AFTER), newest=NEWEST)
        self.assertEqual((code, deployed, named), (0, [["eg"]], []))

    def test_a_dispatch_retry_is_bounded_like_the_release_it_retries(self):
        later = [(key, "0.10.2", AFTER) for key in ALL]
        code, deployed, named, *_ = self.deploy(ALL, *later, event="workflow_dispatch")
        self.assertEqual((code, deployed, named), (0, [ALL], []))
        # A retry approved or re-run after a newer release deployed hands over to it.
        code, deployed, named, *_ = self.deploy(ALL, *later, newest=NEWEST, event="workflow_dispatch")
        self.assertEqual((code, deployed, named), (invoke.SUPERSEDED_BY_NEWER_FINAL, [], ALL))

    def test_contract_group_rules_read_the_release_history(self):
        for transition, expected in ((True, [["web"]]), (False, [["runtime", "runtime-omarchy", "web"]])):
            with self.subTest(transition=transition):
                policy = Mock(return_value=Mock(transition=Mock(return_value=transition)))
                code, deployed, named, _, reads = self.deploy(
                    ["am", "runtime", "runtime-omarchy", "web"], ("am", "0.10.2", AFTER), newest=NEWEST,
                    policy=policy)
                self.assertEqual((code, deployed, named, reads), (0, expected, [], []))
                self.assertEqual(policy.call_args.args[0].head, NEWEST)

    def test_staging_still_counts_every_newer_final(self):
        # The inputs a production release deploys: staging hands an independent set over...
        later = [("eg", "0.10.2", AFTER), ("web", "0.10.2", AFTER)]
        code, deployed, named, bounds, reads = self.deploy(["eg", "web"], *later, target=None)
        self.assertEqual((code, deployed, named, reads),
                         (invoke.SUPERSEDED_BY_NEWER_FINAL, [], ["eg", "web"], [AFTER, AFTER]))
        bounds.assert_not_called()
        # ...deploys only its current members, and still fails a superseded contract member.
        code, deployed, named, bounds, _ = self.deploy(["eg", "web"], later[1], target=None)
        self.assertEqual((code, deployed, named), (0, [["eg"]], []))
        bounds.assert_not_called()
        code, deployed, named, bounds, _ = self.deploy(["am", "web"], ("am", "0.10.2", AFTER), later[1], target=None)
        self.assertEqual((code, deployed, named), ("authority", [], []))
        bounds.assert_not_called()


if __name__ == "__main__":
    unittest.main()
