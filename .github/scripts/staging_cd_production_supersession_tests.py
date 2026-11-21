"""Production supersession is measured against main's newest release, never develop's head."""
import os
import subprocess
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

import prod_cd_select as select
from staging_cd_common import Refusal
from staging_cd_history import DevelopHistory
from staging_cd_overtaken_deploy_tests import receipt
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


if __name__ == "__main__":
    unittest.main()
