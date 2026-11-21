"""The release plan starts at the oldest revision a current lane can still need."""
import contextlib
import copy
import io
import json
import os
from pathlib import Path
import random
import re
import subprocess
import tempfile
import unittest
from unittest.mock import Mock, patch

from ci_tracks.git_comparison import comparison_details, shallow
from staging_cd_behind_fixture import Develop
from staging_cd_common import Refusal
from staging_cd_compatibility import GROUP, RECIPE, VERSION_FILE
from staging_cd_contract_tests import fixture_source
from staging_cd_history import DevelopHistory
from staging_cd_plan_window import MANIFEST, PlanRefused, newest_finals, plan_window, read_finals, window_line
import staging_cd_queue as queue
from staging_cd_queue import release_queue
from staging_cd_receipts import annotation, final_tag_oid, parse_annotation
from staging_cd_receipts_tests import fixture_receipt, fixture_runtime_receipt
from staging_cd_versions import DEPLOYABLES

COMMITS = [f"{index:040x}" for index in range(1, 21)]
LANES = sorted(DEPLOYABLES)
GROUP_IMAGES = sorted(DEPLOYABLES[key] for key in GROUP)
ALL_IMAGES = sorted(DEPLOYABLES.values())
# Seeds of the randomized lockstep; CD_PLAN_LOCKSTEP_SEEDS runs more locally.
SEEDS = int(os.environ.get("CD_PLAN_LOCKSTEP_SEEDS", "3"))


def history(count=10):
    commits = COMMITS[:count]
    return DevelopHistory(commits[-1], list(reversed(commits)))


def finals(**sources):
    return {key: (sources[key], "final") if key in sources else (None, "new") for key in LANES}


def canonical(receipt):
    """The final tag object GitHub serves for a receipt, under its canonical ID."""
    oid = final_tag_oid(receipt)
    return oid, {"sha": oid, "tag": receipt["tag"], "message": annotation(receipt),
                 "object": {"type": "commit", "sha": receipt["source"]}}


def final(component, source, version):
    make = fixture_runtime_receipt if component.startswith("runtime") else fixture_receipt
    major, minor, patch_level = version.split(".")
    receipt = make(component=component, source=source, version=version,
                   previous=f"{major}.{minor}.{int(patch_level) - 1}")
    return ("refs/tags/" + receipt["tag"], *canonical(receipt))


class Tags:
    """A live advertisement and the tag objects GitHub serves for it."""

    def __init__(self, sources=(), develop=COMMITS[9]):
        self.refs, self.objects = {"refs/heads/develop": develop}, {}
        for component, versions in dict(sources).items():
            for version, source in versions.items():
                self.add(*final(component, source, version))

    def add(self, ref, oid, tag):
        self.refs[ref] = oid
        self.objects[oid] = tag

    def request(self, path):
        oid = path.removeprefix("git/tags/")
        if oid not in self.objects:
            raise Refusal("GitHub API request failed")
        return copy.deepcopy(self.objects[oid])

    def advertisement(self):
        return "".join(oid + "\t" + ref + "\n" for ref, oid in self.refs.items())


class NewestFinalTests(unittest.TestCase):
    def tags(self):
        # Each lane has an older and a newer final; only the newer one bounds it.
        return Tags({key: {"0.10.1": COMMITS[1], "0.10.2": COMMITS[3 + index]} for index, key in enumerate(LANES)})

    def test_each_lane_is_bounded_by_its_newest_verified_final(self):
        tags = self.tags()
        self.assertEqual(newest_finals(tags, tags.refs),
                         {key: (COMMITS[3 + index], "final") for index, key in enumerate(LANES)})

    def test_versions_order_finals_not_their_creation_or_name_order(self):
        tags = Tags({"web": {"0.10.9": COMMITS[2], "0.10.10": COMMITS[5]}})
        self.assertEqual(newest_finals(tags, tags.refs)["web"], (COMMITS[5], "final"))

    def test_lane_without_a_final_or_with_only_reservations_is_new(self):
        tags = Tags({"web": {"0.10.1": COMMITS[2]}})
        tags.refs["refs/tags/cma-event-gateway/pending/v0.10.1"] = "a" * 40
        result = newest_finals(tags, tags.refs)
        self.assertEqual(result, dict(dict.fromkeys(LANES, (None, "new")), web=(COMMITS[2], "final")))

    def test_malformed_version_ref_makes_every_lane_untrusted(self):
        for ref in ("refs/tags/cma-web/v1.2", "refs/tags/cma-web/v01.2.3", "refs/tags/cma-runtime/vnext"):
            with self.subTest(ref=ref):
                tags = self.tags()
                tags.refs[ref] = "b" * 40
                self.assertEqual(newest_finals(tags, tags.refs), dict.fromkeys(LANES, (None, "untrusted")))

    def test_an_unreadable_or_untrusted_newest_final_never_falls_back_to_an_older_one(self):
        ref, oid, tag = final("web", COMMITS[6], "0.10.3")
        receipt = json.loads(tag["message"])
        other = fixture_receipt(component="web", source=COMMITS[7], version="0.10.3", previous="0.10.2")
        cases = {
            "unreadable object": (None, "unreadable"),
            "object for another ref": (dict(tag, tag="cma-web/v0.10.2"), "untrusted"),
            "lightweight target": (dict(tag, object={"type": "tree", "sha": COMMITS[6]}), "untrusted"),
            "receipt for another commit": (dict(tag, object={"type": "commit", "sha": COMMITS[7]}), "untrusted"),
            # Self-consistent, but not the object this ID names.
            "substituted object": (dict(tag, message=annotation(other), object={"type": "commit", "sha": COMMITS[7]}),
                                   "untrusted"),
            "not a receipt": (dict(tag, message="release"), "untrusted"),
            "altered receipt": (dict(tag, message=annotation(dict(receipt, digest="sha256:" + "0" * 64))), "untrusted"),
        }
        for name, (served, kind) in cases.items():
            with self.subTest(name):
                tags = self.tags()
                tags.refs[ref] = oid
                if served is not None:
                    tags.objects[oid] = served
                result = newest_finals(tags, tags.refs)
                self.assertEqual(result["web"], (None, kind))
                self.assertEqual(result["eg"], (COMMITS[4], "final"))

    def test_reader_sees_unreadable_inventory_credentials_and_stale_runs(self):
        chain, tags = history(), self.tags()
        failed = Mock(return_value=subprocess.CompletedProcess([], 128, "", "fatal"))
        self.assertEqual(read_finals(Path.cwd(), "fixture", chain, runner=failed),
                         (None, dict.fromkeys(LANES, (None, "unreadable"))))
        listed = Mock(return_value=subprocess.CompletedProcess([], 0, tags.advertisement(), ""))
        for token in (None, "", "two words"):
            with self.subTest(token=token):
                self.assertEqual(read_finals(Path.cwd(), token, chain, runner=listed),
                                 (None, dict.fromkeys(LANES, (None, "unreadable"))))
        # A newer develop head than this plan's makes the run stale: no tag object is read.
        newer = Tags(develop=COMMITS[15])
        listed = Mock(return_value=subprocess.CompletedProcess([], 0, newer.advertisement(), ""))
        with patch("staging_cd_plan_window.GitHub", side_effect=AssertionError):
            self.assertEqual(read_finals(Path.cwd(), "fixture", chain, runner=listed), (COMMITS[15], None))
        # A develop head this plan already contains is not newer: its finals are read.
        for develop in (COMMITS[9], COMMITS[7]):
            with self.subTest(develop=develop):
                tags = Tags(develop=develop)
                listed = Mock(return_value=subprocess.CompletedProcess([], 0, tags.advertisement(), ""))
                self.assertEqual(read_finals(Path.cwd(), "fixture", chain, runner=listed),
                                 (develop, dict.fromkeys(LANES, (None, "new"))))


def unknown(_name):
    raise AssertionError("no lane is new")


class PlanWindowTests(unittest.TestCase):
    def plan(self, adoption, sources, introduced=unknown, count=10):
        return plan_window(history(count), adoption, sources, introduced)

    def test_window_starts_at_the_oldest_lane_newest_final(self):
        sources = finals(**{"am": COMMITS[4], "eg": COMMITS[6], "runtime": COMMITS[4],
                            "runtime-omarchy": COMMITS[5], "web": COMMITS[8]})
        self.assertEqual(self.plan(COMMITS[1], sources), (COMMITS[4], ["am", "runtime"], {}))

    def test_the_adoption_commit_is_not_needed_once_every_lane_has_a_final(self):
        sources = finals(**dict({key: COMMITS[6] for key in LANES}, web=COMMITS[3]))
        for adoption in (None, "", "f" * 40):  # Unset, or older than this history.
            with self.subTest(adoption=adoption):
                self.assertEqual(self.plan(adoption, sources), (COMMITS[3], ["web"], {}))

    def test_an_adoption_newer_than_the_oldest_final_is_still_the_start(self):
        sources = finals(**{key: COMMITS[2] for key in LANES})
        self.assertEqual(self.plan(COMMITS[6], sources), (COMMITS[6], ["adoption"], {}))
        self.assertEqual(self.plan(COMMITS[2], sources), (COMMITS[2], LANES, {}))

    def test_a_lane_that_never_had_a_final_is_bounded_by_its_introduction(self):
        sources = finals(**{key: COMMITS[6] for key in LANES if key != "web"})
        introduced = Mock(return_value=COMMITS[3])
        self.assertEqual(self.plan(None, sources, introduced), (COMMITS[3], ["web:new"], {"web": COMMITS[3]}))
        introduced.assert_called_once_with("cma-web")
        # An introduction before the adoption commit leaves the adoption commit as the start.
        self.assertEqual(self.plan(COMMITS[4], sources, introduced), (COMMITS[4], ["adoption"], {"web": COMMITS[3]}))

    def test_only_a_lane_without_a_bound_on_this_history_needs_the_adoption_floor(self):
        sources = finals(**{key: COMMITS[6] for key in LANES})
        for name, lane in (("final off history", ("f" * 40, "final")), ("introduced off history", (None, "new"))):
            with self.subTest(name):
                bounded = dict(sources, web=lane)
                introduced = Mock(return_value=None)
                self.assertEqual(self.plan(COMMITS[2], bounded, introduced), (COMMITS[2], ["web:floor"], {}))
                for adoption in (None, "f" * 40):
                    with self.assertRaises(PlanRefused) as refused:
                        self.plan(adoption, bounded, introduced)
                    self.assertEqual((refused.exception.kind, refused.exception.lanes), ("floor", ["web"]))

    def test_an_unreadable_or_untrusted_final_refuses_the_plan_at_once(self):
        sources = finals(**{key: COMMITS[6] for key in LANES})
        for kinds, expected in (({"eg": "unreadable"}, ("unreadable", ["eg"])),
                                ({"eg": "untrusted", "web": "untrusted"}, ("untrusted", ["eg", "web"])),
                                ({"eg": "unreadable", "web": "untrusted"}, ("untrusted", ["web"]))):
            with self.subTest(kinds=kinds):
                with self.assertRaises(PlanRefused) as refused:
                    self.plan(COMMITS[1], dict(sources, **{key: (None, kind) for key, kind in kinds.items()}))
                self.assertEqual((refused.exception.kind, refused.exception.lanes), expected)
        with self.assertRaises(PlanRefused) as refused:
            self.plan(COMMITS[1], {})
        self.assertEqual((refused.exception.kind, refused.exception.lanes), ("unreadable", LANES))

    def test_the_window_never_starts_before_adoption_or_after_any_lane_bound(self):
        chain, rng = history(12), random.Random(7)
        candidates = [None, "f" * 40, *COMMITS[:12]]
        for _ in range(500):
            adoption = rng.choice([None, "f" * 40, *COMMITS[:12]])
            sources = {key: (rng.choice(candidates), rng.choice(["final", "new"])) for key in LANES}
            introductions = {DEPLOYABLES[key]: source for key, (source, kind) in sources.items() if kind == "new"}
            sources = {key: (None, "new") if kind == "new" else (source, kind) for key, (source, kind) in sources.items()}
            floor = chain.positions.get(adoption)
            bounds = [chain.positions.get(introductions.get(DEPLOYABLES[key], source), floor)
                      for key, (source, _kind) in sources.items()]
            try:
                start, lanes, _relied = plan_window(chain, adoption, sources, introductions.get)
            except PlanRefused as refused:
                self.assertEqual(refused.kind, "floor")
                self.assertIsNone(floor)
                self.assertIn(None, bounds)
                continue
            self.assertNotIn(None, bounds)
            self.assertEqual(chain.position(start), max(min(bounds), floor if floor is not None else min(bounds)))
            self.assertTrue(lanes)

    def test_window_line_names_why_the_plan_covers_what_it_does(self):
        line = window_line({"start": COMMITS[4], "revisions": 6, "adoption": COMMITS[1], "bound_by": ["am", "eg"]})
        self.assertEqual(line, f"PLAN window start={COMMITS[4]} revisions=6 adoption={COMMITS[1]} bound-by=am,eg")
        line = window_line({"start": COMMITS[4], "revisions": 6, "adoption": "none", "bound_by": ["am"],
                            "transition": COMMITS[2]})
        self.assertEqual(line, f"PLAN window start={COMMITS[4]} revisions=6 adoption=none bound-by=am "
                               f"transition={COMMITS[2]}")
        self.assertEqual(window_line({"start": COMMITS[4], "refused": "floor", "lanes": ["eg", "web"]}),
                         "PLAN refused reason=floor lanes=eg,web")
        self.assertEqual(window_line({"stale": COMMITS[9], "head": COMMITS[4]}),
                         f"PLAN stale develop={COMMITS[9]} head={COMMITS[4]}")


class Repository:
    """A real first-parent chain: a root, a base holding the deployable manifest,
    then `count` merges. With `contract_at` the base also declares the previous
    runtime contract and that merge is a contract transition. `added` maps
    merge indexes to the lanes they add to the manifest. The shared selector is
    replaced by fixed decisions, and its calls are recorded."""

    def __init__(self, test, count, *, contract_at=None, added=None):
        directory = tempfile.TemporaryDirectory()
        test.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.run("init", "--quiet")
        self.run("commit", "--allow-empty", "-qm", "Root")
        added = added or {}
        names = sorted(set(DEPLOYABLES.values()) - {name for later in added.values() for name in later})
        current = {}
        if contract_at is not None:
            fixture_source(self.root)
            current = {path: (self.root / path).read_text() for path in (RECIPE, VERSION_FILE)}
            for path, content in current.items():
                (self.root / path).write_text(content.replace('"3"', '"2"'))
        self.manifest(names)
        self.base = self.commit("Base")
        self.commits = []
        for index in range(count):
            if index == contract_at:
                for path, content in current.items():
                    (self.root / path).write_text(content)
            if index in added:
                names = sorted(names + added[index])
                self.manifest(names)
            self.commits.append(self.commit(f"Merge {index}"))
        self.decisions, self.calls, self.clock = {}, [], None

    def run(self, *args):
        return subprocess.run(["git", "-c", "user.name=Test", "-c", "user.email=test@example.invalid",
                               "-c", "commit.gpgSign=false", *args], cwd=self.root, check=True,
                              capture_output=True, text=True, timeout=10).stdout.strip()

    def manifest(self, names):
        path = self.root / MANIFEST
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps({name: {} for name in names}))

    def commit(self, message):
        self.run("add", "-A")
        self.run("commit", "--allow-empty", "-qm", message)
        return self.run("rev-parse", "HEAD")

    def tree(self, commit):
        return self.run("rev-parse", commit + "^{tree}")

    def batch(self, _root, revisions, _metrics, budget, cached=True):
        self.calls.append((list(revisions), budget, cached))
        if self.clock is not None:
            self.clock[0] += 10
        return {revision: {"deployables": self.decisions.get(revision, [])} for revision in revisions}

    def row(self, commit):
        return {"source": commit, "tree": self.tree(commit), "deployables": self.decisions.get(commit, [])}

    def plan(self, adoption, *, lanes=None, develop=None, head=None, budget=None):
        """Plan from the adoption commit, or bounded by `lanes` (each lane's newest final)."""
        found, head = {}, head or self.commits[-1]
        reader = None if lanes is None else (lambda _history: (develop or head, lanes))
        with patch.object(queue, "batch_selection", side_effect=self.batch):
            result = release_queue(self.root, adoption, head, head, {}, budget, finals=reader, window=found)
        return result, found


def windows(calls, cached=True):
    return [revisions for revisions, _budget, flag in calls if flag is cached]


class ReleaseQueueWindowTests(unittest.TestCase):
    def test_without_finals_the_plan_is_the_adoption_window(self):
        repo = Repository(self, 6)
        repo.decisions = {repo.commits[2]: ["cma-web"], repo.commits[4]: ["cma-agent-manager"]}
        before, found = repo.plan(repo.commits[1])
        self.assertEqual((windows(repo.calls), found), ([repo.commits[1:]], {}))
        self.assertEqual(before["changes"], [repo.row(repo.commits[2]), repo.row(repo.commits[4])])
        # Lanes that never had a final are bounded by their introduction, here
        # before the adoption commit, so the adoption window is planned again
        # and its first revision is listed too.
        repo.calls.clear()
        after, found = repo.plan(repo.commits[1], lanes=dict.fromkeys(LANES, (None, "new")))
        self.assertEqual(windows(repo.calls), [repo.commits[1:]])
        self.assertEqual(after, dict(before, changes=[repo.row(repo.commits[1]), *before["changes"]]))
        self.assertEqual(found, {"start": repo.commits[1], "revisions": 5, "adoption": repo.commits[1],
                                 "bound_by": ["adoption"]})

    def test_bounded_plan_selects_only_the_window_and_lists_its_first_revision(self):
        repo = Repository(self, 7)
        repo.decisions = {repo.commits[2]: ["cma-web"], repo.commits[4]: ["cma-web"], repo.commits[5]: ["cma-web"]}
        lanes = finals(**dict({key: repo.commits[3] for key in LANES}, web=repo.commits[4]))
        for adoption in (repo.commits[1], None):
            with self.subTest(adoption=adoption):
                repo.calls.clear()
                result, found = repo.plan(adoption, lanes=lanes)
                self.assertEqual(windows(repo.calls), [repo.commits[3:]])
                # The window's first revision selects nothing now, yet allocation must see it.
                self.assertEqual(result["changes"], [repo.row(repo.commits[3]), repo.row(repo.commits[4]),
                                                     repo.row(repo.commits[5])])
                self.assertEqual(found, {"start": repo.commits[3], "revisions": 4, "adoption": adoption or "none",
                                         "bound_by": [key for key in LANES if key != "web"]})

    def test_a_stale_run_plans_nothing_and_reads_no_selection(self):
        repo, found = Repository(self, 4), {}
        reader = Mock(return_value=("f" * 40, None))
        with patch.object(queue, "batch_selection", side_effect=repo.batch):
            result = release_queue(repo.root, repo.commits[0], repo.commits[-1], repo.commits[-1], {},
                                   finals=reader, window=found)
        self.assertEqual((result, repo.calls), ({"head": repo.commits[-1], "event_source": repo.commits[-1],
                                                 "changes": []}, []))
        self.assertEqual(found, {"stale": "f" * 40, "head": repo.commits[-1]})

    def test_a_refused_window_reads_no_selection(self):
        repo = Repository(self, 4)
        with self.assertRaises(PlanRefused) as refused:
            repo.plan(None, lanes=dict(finals(**{key: repo.commits[2] for key in LANES}), eg=(None, "unreadable")))
        self.assertEqual((refused.exception.kind, refused.exception.lanes, repo.calls), ("unreadable", ["eg"], []))
        with self.assertRaisesRegex(Refusal, "immutable commits"):
            repo.plan("not-a-commit", lanes=finals(**{key: repo.commits[2] for key in LANES}))

    def test_the_finals_reader_runs_only_after_develop_history_is_bound(self):
        repo = Repository(self, 4)
        reader = Mock(side_effect=AssertionError)
        with self.assertRaisesRegex(Refusal, "no longer on develop"), patch.object(queue, "batch_selection"):
            release_queue(repo.root, repo.commits[0], repo.commits[-1], "f" * 40, {}, finals=reader)
        reader.assert_not_called()

    def test_a_transition_older_than_every_final_is_selected_alone_before_the_window(self):
        repo = Repository(self, 6, contract_at=1)
        old, transition, *later = repo.commits
        repo.decisions = {transition: ALL_IMAGES, later[0]: GROUP_IMAGES,
                          later[1]: ["cma-event-gateway", "cma-web"], later[3]: ["cma-agent-manager"]}
        lanes = finals(**{key: later[0] for key in GROUP}, eg=later[1], web=later[1])
        full, _found = repo.plan(old)
        self.assertEqual(full["contract_transition"], transition)
        repo.calls.clear()
        bounded, found = repo.plan(old, lanes=lanes, budget=600)
        # Resolved before the window, uncached so the window's records are never pruned.
        self.assertEqual([(revisions, cached) for revisions, _budget, cached in repo.calls],
                         [([transition], False), (later, True)])
        self.assertEqual(bounded, full)
        self.assertEqual((found["start"], found["revisions"], found["transition"]), (later[0], 4, transition))
        # The adoption commit still keeps older transitions out.
        repo.calls.clear()
        bounded, found = repo.plan(later[0], lanes=lanes)
        self.assertEqual(windows(repo.calls, cached=False), [])
        self.assertNotIn("contract_transition", bounded)

    def test_a_transition_inside_the_window_needs_no_separate_selection(self):
        repo = Repository(self, 4, contract_at=1)
        old, transition, *later = repo.commits
        repo.decisions = {transition: GROUP_IMAGES, later[0]: ["cma-web"]}
        bounded, found = repo.plan(old, lanes=finals(**{key: transition for key in LANES}))
        self.assertEqual(bounded["contract_transition"], transition)
        self.assertEqual([(revisions, cached) for revisions, _budget, cached in repo.calls],
                         [(repo.commits[1:], True)])
        self.assertNotIn("transition", found)

    def test_an_unselected_or_unreadable_earlier_transition_refuses_before_the_window(self):
        repo = Repository(self, 5, contract_at=1)
        old, transition, *later = repo.commits
        repo.decisions = {transition: ["cma-web"], later[1]: ["cma-web"]}
        lanes = finals(**{key: later[0] for key in LANES})
        with self.assertRaises(PlanRefused) as refused:
            repo.plan(old, lanes=lanes)
        self.assertEqual((refused.exception.kind, refused.exception.lanes), ("anchor", sorted(GROUP)))
        self.assertEqual(windows(repo.calls), [])  # No window batch pruned the cache.
        repo.calls.clear()
        unavailable = Refusal("Runtime contract source is unavailable")
        with patch("staging_cd_plan_window.ContractPolicy.transition", side_effect=unavailable), \
                self.assertRaisesRegex(Refusal, "contract source"):
            repo.plan(old, lanes=lanes)
        self.assertEqual(repo.calls, [])

    def test_a_new_lane_is_bounded_by_the_merge_that_added_it(self):
        repo = Repository(self, 6, added={2: ["cma-web"]})
        repo.decisions = {repo.commits[2]: ["cma-web"], repo.commits[4]: ["cma-agent-manager"]}
        lanes = dict(finals(**{key: repo.commits[4] for key in LANES if key != "web"}))
        result, found = repo.plan(None, lanes=lanes)
        self.assertEqual(windows(repo.calls), [repo.commits[2:]])
        self.assertEqual((found["start"], found["bound_by"]), (repo.commits[2], ["web:new"]))
        self.assertEqual(result["changes"], [repo.row(repo.commits[2]), repo.row(repo.commits[4])])
        # A lane the window never selects from its introduction on refuses the plan.
        repo.decisions = {repo.commits[4]: ["cma-agent-manager"]}
        with self.assertRaises(PlanRefused) as refused:
            repo.plan(None, lanes=lanes)
        self.assertEqual((refused.exception.kind, refused.exception.lanes), ("anchor", ["web"]))
        # Without any manifest introduction on this history the lane needs the floor.
        repo = Repository(self, 3)
        with patch.object(queue, "introduction", return_value=None), \
                self.assertRaises(PlanRefused) as refused:
            repo.plan(None, lanes=dict(finals(**{key: repo.commits[1] for key in LANES if key != "web"})))
        self.assertEqual((refused.exception.kind, refused.exception.lanes), ("floor", ["web"]))

    def test_every_selection_shares_one_budget_with_the_tag_reads(self):
        repo = Repository(self, 6, contract_at=1)
        old, transition, *later = repo.commits
        repo.decisions = {transition: GROUP_IMAGES, later[1]: ["cma-web"]}
        repo.clock = clock = [1000]

        def reader(_history):
            clock[0] += 40  # Reading the tags takes 40 s of the 100 s budget.
            return later[-1], finals(**{key: later[0] for key in LANES})
        with patch.object(queue, "batch_selection", side_effect=repo.batch), \
                patch.object(queue.time, "monotonic", side_effect=lambda: clock[0]):
            release_queue(repo.root, old, later[-1], later[-1], {}, 100, finals=reader)
        self.assertEqual([(budget, cached) for _revisions, budget, cached in repo.calls], [(60, False), (50, True)])

    def test_the_anchor_is_compared_with_its_parent_even_in_a_shallow_checkout(self):
        # The single-revision selector reports shallow-history for every merge
        # of a shallow checkout; the batch selector the anchor now uses does not.
        repo = Repository(self, 4, contract_at=2)
        with tempfile.TemporaryDirectory() as directory:
            clone = Path(directory) / "clone"
            subprocess.run(["git", "clone", "--quiet", "--depth", "3", repo.root.as_uri(), str(clone)],
                           check=True, capture_output=True, timeout=30)
            self.assertTrue(shallow(clone))
            anchor = repo.commits[2]
            self.assertEqual(comparison_details(clone, {"after": anchor}, "push")[2], "shallow-history")
            self.assertEqual(comparison_details(clone, {"after": anchor}, "push", allow_shallow_parent=True),
                             (repo.commits[1], anchor, None))
        selector = (Path(queue.__file__).parent / "ci_tracks/batch_selection.py").read_text()
        self.assertIn("comparison_details(root, {'after': head}, 'push', allow_shallow_parent=True)", selector)


IMAGE = re.compile(r"(cma-[a-z-]+)/v[0-9]+\.[0-9]+\.[0-9]+")


class Lockstep:
    """Two simulated develops receive the same pushes on one real chain.

    One plans from the adoption commit (staging CD before this change), the
    other is bounded by its own newest finals, read as canonical tags through
    newest_finals. Both plans come from release_queue and allocate for real.
    """

    def __init__(self, test, steps, *, contract_at=None):
        self.test = test
        self.repo = Repository(test, steps + 1, contract_at=contract_at)
        self.full, self.bounded = Develop(steps), Develop(steps)
        for develop in (self.full, self.bounded):
            develop.server.sources = list(self.repo.commits)
            develop.server._history = DevelopHistory(self.repo.commits[-1], list(reversed(self.repo.commits)))
        self.full.plan = lambda planned: self.plan(planned, None)
        self.bounded.plan = lambda planned: self.plan(planned, self.bounded)
        self.windows, self.extra = [], []

    def plan(self, planned, develop):
        head = planned["head"]
        reader = None if develop is None else (lambda _history: (head, self.finals(develop)))
        found = {}
        with patch.object(queue, "batch_selection", side_effect=self.repo.batch):
            result = release_queue(self.repo.root, self.repo.commits[0], head, head, {}, finals=reader, window=found)
        if develop is not None:
            self.windows.append((found["start"], len(result["changes"])))
        return result

    @staticmethod
    def finals(develop):
        tags = Tags(develop=develop.head)
        for name, oid in develop.server.refs.items():
            match = IMAGE.fullmatch(name)
            if match and match[1] in DEPLOYABLES.values():
                tags.add("refs/tags/" + name, *canonical(parse_annotation(develop.server.objects[oid]["message"])))
            else:
                tags.refs["refs/tags/" + name] = oid
        return newest_finals(tags, tags.refs)

    def push(self, components, *, refusal=False, **kwargs):
        """Push to both; with `refusal`, an identical refusal of both ends the run (None)."""
        index = len(self.full.runs)
        self.repo.decisions[self.repo.commits[index]] = sorted(DEPLOYABLES[key] for key in components)
        outcomes = []
        for develop in (self.full, self.bounded):
            try:
                outcomes.append(develop.push(components, **kwargs))
            except Refusal as error:
                if not refusal:
                    raise
                outcomes.append(str(error))
        if any(isinstance(outcome, str) for outcome in outcomes):
            self.test.assertEqual(outcomes[0], outcomes[1])
            return None
        expected, actual = outcomes
        # Everything the adoption window plans, the bounded plan plans identically.
        self.test.assertEqual({key: actual.get(key) for key in expected}, expected)
        extra = {key: task for key, task in actual.items() if key not in expected}
        self.test.assertTrue(all(task["mode"] == "resume" for task in extra.values()), extra)
        self.extra.append(sorted(extra))
        return expected, actual

    def finalize(self, tasks, *components):
        for develop in (self.full, self.bounded):
            for component in components:
                develop.finalize(tasks[component])

    def complete(self, run_id):
        for develop in (self.full, self.bounded):
            develop.complete(run_id)

    def unselect(self, index):
        self.repo.decisions[self.repo.commits[index]] = []
        for develop in (self.full, self.bounded):
            develop.unselect(index)

    def same_tags(self):
        self.test.assertEqual(self.bounded.server.refs, self.full.server.refs)


class AllocationLockstepTests(unittest.TestCase):
    def test_bounded_plans_allocate_like_the_adoption_window(self):
        step = Lockstep(self, 10)
        source = step.repo.commits
        step.push(set())
        step.complete(801)
        step.finalize(step.push({"web"})[0], "web")
        step.complete(802)
        step.finalize(step.push({"eg"})[0], "eg")
        step.complete(803)
        # Until every lane has a final, lanes are bounded by their introduction,
        # before the adoption commit, so the adoption commit is the start.
        step.finalize(step.push(set(LANES))[0], *LANES)
        step.complete(804)
        web, _ = step.push({"web"})
        self.assertEqual(web.keys(), set(LANES))  # The others resume their unproven finals.
        step.finalize(web, "web")
        step.complete(805)
        # The gateway build fails; a later publisher-only merge retries it at its source.
        self.assertEqual(step.push({"eg"})[0]["eg"]["mode"], "publish")
        step.complete(806)
        retry, _ = step.push(set(), verified={"web", "am", "runtime", "runtime-omarchy"})
        self.assertEqual((retry.keys(), retry["eg"]["reservation"]["source"]), ({"eg"}, source[5]))
        step.finalize(retry, "eg")
        step.complete(807)
        step.unselect(4)  # A narrower selector no longer selects web's newest final.
        agent, _ = step.push({"am"}, verified={"eg"})
        self.assertEqual(agent.keys(), set(LANES) - {"eg"})
        step.finalize(agent, "am")
        step.complete(808)
        runtimes, _ = step.push({"runtime", "runtime-omarchy"}, verified={"eg", "am", "web"})
        step.finalize(runtimes, "runtime", "runtime-omarchy")
        step.complete(809)
        # Web's unselected newest final now bounds the window and still resumes.
        resumed, _ = step.push(set(), verified={"eg", "am", "runtime", "runtime-omarchy"})
        self.assertEqual(resumed, {"web": {"component": "web", "mode": "resume", "receipt": step.full.finals["web"]}})
        step.complete(810)
        self.assertEqual(step.push(set(), verified=set(LANES)), ({}, {}))
        step.same_tags()
        self.assertEqual([start for start, _rows in step.windows],
                         [source[0]] * 4 + [source[3]] * 5 + [source[4]] * 2)
        self.assertEqual(step.windows[-1][1], 4)
        self.assertEqual(len(step.full.queues[811]["changes"]), 6)
        self.assertEqual(step.extra, [[]] * 11)

    def test_the_bounded_plan_also_resumes_finals_the_adoption_window_strands(self):
        # Once a narrower selector stops selecting the merge every lane's newest
        # final came from, the adoption window lists no row at or before it and
        # resumes nothing; the bounded window starts there and resumes them all.
        step = Lockstep(self, 4)
        step.push(set())
        step.complete(801)
        step.finalize(step.push(set(LANES))[0], *LANES)
        step.complete(802)
        step.unselect(1)
        expected, actual = step.push(set())
        self.assertEqual(expected, {})
        self.assertEqual(actual, {key: {"component": key, "mode": "resume", "receipt": step.full.finals[key]}
                                  for key in LANES})
        self.assertEqual(step.extra[-1], LANES)
        step.same_tags()

    def test_an_active_owner_keeps_its_lane_busy_in_both_plans(self):
        step = Lockstep(self, 6)
        step.push(set())
        step.complete(801)
        step.finalize(step.push(set(LANES))[0], *LANES)
        step.complete(802)
        reserved, _ = step.push({"web"}, verified=set(LANES))  # Run 803 keeps building.
        self.assertEqual(reserved["web"]["mode"], "publish")
        gateway, _ = step.push({"eg"}, verified=set(LANES))
        self.assertNotIn("web", gateway)
        step.complete(803)
        step.complete(804)
        retry, _ = step.push(set(), verified=set(LANES))  # Its owner failed: retry at its source.
        self.assertEqual(retry["web"]["reservation"]["source"], step.repo.commits[2])
        step.same_tags()

    def test_a_transition_older_than_every_final_still_couples_its_group(self):
        step = Lockstep(self, 6, contract_at=1)
        step.push(set())
        step.complete(801)
        group, _ = step.push(set(GROUP))
        self.assertEqual(step.full.queues[802]["contract_transition"], step.repo.commits[1])
        step.finalize(group, *GROUP)
        step.complete(802)
        step.finalize(step.push({"web", "eg"}, verified=set(GROUP))[0], "web", "eg")
        step.complete(803)
        step.finalize(step.push(set(LANES), verified=set(LANES))[0], *LANES)
        step.complete(804)
        # Every lane's newest final is newer than the transition: the bounded
        # window starts after it, lists it, and still resumes the peers.
        successor, _ = step.push({"am"}, verified=set(LANES) - {"am"})
        self.assertEqual(successor.keys(), GROUP)
        self.assertEqual(step.bounded.queues[805]["contract_transition"], step.repo.commits[1])
        self.assertEqual(step.windows[-1][0], step.repo.commits[3])
        step.same_tags()

    def test_randomized_pushes_never_skip_what_the_adoption_window_plans(self):
        for seed in range(SEEDS):
            with self.subTest(seed=seed):
                self.lockstep(seed)

    def lockstep(self, seed, steps=12):
        rng = random.Random(seed)
        contract_at = rng.choice([None, 1, 3])
        step = Lockstep(self, steps, contract_at=contract_at)
        active = []
        for index in range(steps):
            components = set(rng.sample(LANES, rng.choice([0, 0, 1, 1, 2, 5])))
            if index == contract_at:
                components |= GROUP  # A contract transition always selects its group.
            verified = set(rng.sample(LANES, rng.randint(0, 5))) & set(step.full.finals)
            pushed = step.push(components, verified=verified, refusal=True)
            if pushed is None:
                return
            tasks = pushed[0]
            run_id = 801 + index
            fate = rng.choice(["finalize", "finalize", "fail", "running"])
            if fate == "finalize":
                step.finalize(tasks, *[key for key, task in tasks.items() if task["mode"] == "publish"])
            if fate == "running":
                active.append(run_id)
            else:
                step.complete(run_id)
            if active and rng.random() < 0.5:
                step.complete(active.pop(0))
            if index > 2 and rng.random() < 0.15:
                unselected = rng.randrange(1, index + 1)
                if unselected != contract_at:
                    step.unselect(unselected)
        step.same_tags()


class MainWindowTests(unittest.TestCase):
    def run_main(self, plan, argv=(), extra=None):
        with tempfile.TemporaryDirectory() as directory:
            outputs = Path(directory) / "outputs"
            outputs.write_text("")
            env = {"GITHUB_OUTPUT": str(outputs), "GITHUB_EVENT_NAME": "push", "GITHUB_SHA": COMMITS[3],
                   "GH_TOKEN": "synthetic-not-a-credential", "CD_ADOPTION_SHA": COMMITS[0], **(extra or {})}
            stdout = io.StringIO()
            with patch.dict(os.environ, env), patch.object(queue, "event_identity"), \
                    patch.object(queue, "git", return_value=COMMITS[3]), \
                    patch.object(queue, "read_finals", return_value="read") as reader, \
                    patch.object(queue, "release_queue", side_effect=plan), contextlib.redirect_stdout(stdout):
                status = queue.main(list(argv))
            return status, outputs.read_text(), stdout.getvalue(), reader

    def test_main_reads_finals_lazily_and_prints_the_chosen_window(self):
        def plan(root, adoption, head, event_source, metrics, *, budget, finals, window):
            self.assertEqual((adoption, head, event_source, budget), (COMMITS[0], COMMITS[3], COMMITS[3],
                                                                     queue.BUDGET_SECONDS))
            self.assertEqual(finals("history"), "read")
            window.update(start=COMMITS[2], revisions=2, adoption=adoption, bound_by=["eg"])
            return {"head": head, "event_source": event_source, "changes": []}
        status, outputs, stdout, reader = self.run_main(plan)
        self.assertEqual(status, 0)
        reader.assert_called_once_with(Path.cwd(), "synthetic-not-a-credential", "history")
        self.assertIn(f"PLAN window start={COMMITS[2]} revisions=2 adoption={COMMITS[0]} bound-by=eg\n", stdout)

    def test_a_refused_plan_saves_nothing_planned_and_its_gate_names_the_reason(self):
        def plan(*_args, **_kwargs):
            raise PlanRefused("floor", ["eg"])
        status, outputs, stdout, _reader = self.run_main(plan)
        self.assertEqual((status, outputs), (0, "complete=false\nrefused=floor\n"))
        self.assertIn("PLAN refused reason=floor lanes=eg\n", stdout)
        for kind, code in queue.PLAN_REFUSALS.items():
            with self.subTest(kind=kind):
                status, _outputs, stdout, _reader = self.run_main(plan, ["--require-complete"],
                                                                  {"CD_PLAN_REFUSED": kind})
                self.assertEqual(status, 1)
                self.assertEqual(stdout.splitlines()[0].split(" cause=")[0], f"FAIL release-queue reason={code}")
        status, _outputs, stdout, _reader = self.run_main(plan, ["--require-complete"], {"CD_PLAN_REFUSED": "other"})
        self.assertTrue(stdout.startswith("FAIL release-queue reason=incomplete"))

    def test_a_stale_run_completes_with_nothing_changed(self):
        def plan(root, adoption, head, event_source, metrics, *, budget, finals, window):
            window.update(stale=COMMITS[9], head=head)
            return {"head": head, "event_source": event_source, "changes": []}
        status, outputs, stdout, _reader = self.run_main(plan)
        self.assertEqual(status, 0)
        self.assertEqual(outputs.splitlines()[1:], ["changed=false", "complete=true"])
        self.assertIn(f"PLAN stale develop={COMMITS[9]} head={COMMITS[3]}\n", stdout)


if __name__ == "__main__":
    unittest.main()
