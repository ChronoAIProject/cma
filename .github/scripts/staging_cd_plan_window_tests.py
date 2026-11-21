"""The release plan starts at the oldest revision a current lane can still need."""
import contextlib
import copy
import io
import json
import os
from pathlib import Path
import random
import subprocess
import tempfile
import unittest
from unittest.mock import Mock, patch

from staging_cd_behind_fixture import Develop
from staging_cd_common import Refusal
from staging_cd_compatibility import GROUP, RECIPE, VERSION_FILE
from staging_cd_contract_tests import fixture_source
from staging_cd_history import DevelopHistory
import staging_cd_plan_window as window
from staging_cd_plan_window import newest_finals, plan_window, read_finals, window_line
import staging_cd_queue as queue
from staging_cd_queue import reconcile, release_queue
from staging_cd_receipts import annotation, final_tag_oid
from staging_cd_receipts_tests import fixture_receipt, fixture_runtime_receipt
from staging_cd_versions import DEPLOYABLES

COMMITS = [f"{index:040x}" for index in range(1, 21)]
LANES = sorted(DEPLOYABLES)
GROUP_IMAGES = sorted(DEPLOYABLES[key] for key in GROUP)
ALL_IMAGES = sorted(DEPLOYABLES.values())


def history(count=10):
    commits = COMMITS[:count]
    return DevelopHistory(commits[-1], list(reversed(commits)))


def finals(**sources):
    return {key: (sources[key], "final") if key in sources else (None, "no-final") for key in LANES}


def final(component, source, version):
    make = fixture_runtime_receipt if component.startswith("runtime") else fixture_receipt
    major, minor, patch_level = version.split(".")
    receipt = make(component=component, source=source, version=version,
                   previous=f"{major}.{minor}.{int(patch_level) - 1}")
    oid = final_tag_oid(receipt)
    tag = {"sha": oid, "tag": receipt["tag"], "message": annotation(receipt),
           "object": {"type": "commit", "sha": source}}
    return "refs/tags/" + receipt["tag"], oid, tag


class Tags:
    """A live advertisement and the tag objects GitHub serves for it."""

    def __init__(self, sources):
        self.refs, self.objects = {"refs/heads/develop": COMMITS[9]}, {}
        for component, versions in sources.items():
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

    def test_lane_without_a_final_or_with_only_reservations_has_no_bound(self):
        tags = Tags({"web": {"0.10.1": COMMITS[2]}})
        tags.refs["refs/tags/cma-event-gateway/pending/v0.10.1"] = "a" * 40
        result = newest_finals(tags, tags.refs)
        self.assertEqual(result["web"], (COMMITS[2], "final"))
        self.assertEqual({key: result[key] for key in LANES if key != "web"},
                         dict.fromkeys(set(LANES) - {"web"}, (None, "no-final")))

    def test_malformed_version_ref_leaves_every_lane_unbounded(self):
        for ref in ("refs/tags/cma-web/v1.2", "refs/tags/cma-web/v01.2.3", "refs/tags/cma-runtime/vnext"):
            with self.subTest(ref=ref):
                tags = self.tags()
                tags.refs[ref] = "b" * 40
                self.assertEqual(newest_finals(tags, tags.refs), dict.fromkeys(LANES, (None, "malformed")))

    def test_an_untrusted_newest_final_never_falls_back_to_an_older_trusted_one(self):
        ref, oid, tag = final("web", COMMITS[6], "0.10.3")
        receipt = json.loads(tag["message"])
        other = fixture_receipt(component="web", source=COMMITS[7], version="0.10.3", previous="0.10.2")
        altered = dict(tag, message=annotation(dict(receipt, digest="sha256:" + "0" * 64)))
        cases = {
            "unreadable object": None,
            "object for another ref": dict(tag, tag="cma-web/v0.10.2"),
            "lightweight target": dict(tag, object={"type": "tree", "sha": COMMITS[6]}),
            "receipt for another commit": dict(tag, object={"type": "commit", "sha": COMMITS[7]}),
            # Self-consistent, but not the object this ID names.
            "substituted object": dict(tag, message=annotation(other), object={"type": "commit", "sha": COMMITS[7]}),
            "not a receipt": dict(tag, message="release"),
            "altered receipt": altered,
        }
        for name, served in cases.items():
            with self.subTest(name):
                tags = self.tags()
                tags.refs[ref] = oid
                if served is not None:
                    tags.objects[oid] = served
                result = newest_finals(tags, tags.refs)
                self.assertEqual(result["web"], (None, "unverified"))
                self.assertEqual(result["eg"], (COMMITS[4], "final"))

    def test_unreadable_inventory_or_credential_leaves_every_lane_unbounded(self):
        failed = Mock(return_value=subprocess.CompletedProcess([], 128, "", "fatal"))
        self.assertEqual(read_finals(Path.cwd(), "fixture", runner=failed), dict.fromkeys(LANES, (None, "unreadable")))
        listed = Mock(return_value=subprocess.CompletedProcess([], 0, "a" * 40 + "\trefs/heads/develop\n", ""))
        for token in (None, "", "two words"):
            with self.subTest(token=token):
                self.assertEqual(read_finals(Path.cwd(), token, runner=listed),
                                 dict.fromkeys(LANES, (None, "unreadable")))
        self.assertEqual(read_finals(Path.cwd(), "fixture", runner=listed), dict.fromkeys(LANES, (None, "no-final")))


class PlanWindowTests(unittest.TestCase):
    def test_window_starts_at_the_oldest_lane_newest_final(self):
        sources = finals(**{"am": COMMITS[4], "eg": COMMITS[6], "runtime": COMMITS[4],
                            "runtime-omarchy": COMMITS[5], "web": COMMITS[8]})
        self.assertEqual(plan_window(history(), COMMITS[1], sources), (COMMITS[4], ["am", "runtime"]))

    def test_a_lane_without_a_final_keeps_the_adoption_commit(self):
        sources = finals(**{key: COMMITS[6] for key in LANES if key != "web"})
        self.assertEqual(plan_window(history(), COMMITS[1], sources), (COMMITS[1], ["web:no-final"]))

    def test_a_final_off_first_parent_history_keeps_the_adoption_commit(self):
        for source in ("f" * 40, COMMITS[15]):  # A side branch, or beyond this plan's head.
            with self.subTest(source=source):
                sources = finals(**dict({key: COMMITS[6] for key in LANES}, web=source))
                self.assertEqual(plan_window(history(), COMMITS[1], sources), (COMMITS[1], ["web:off-history"]))

    def test_an_untrusted_or_unreadable_final_keeps_the_adoption_commit(self):
        for reason in ("unverified", "malformed", "unreadable"):
            with self.subTest(reason=reason):
                sources = dict(finals(**{key: COMMITS[6] for key in LANES}), eg=(None, reason))
                self.assertEqual(plan_window(history(), COMMITS[1], sources), (COMMITS[1], ["eg:" + reason]))
        self.assertEqual(plan_window(history(), COMMITS[1], {}), (COMMITS[1], [key + ":unreadable" for key in LANES]))

    def test_an_adoption_newer_than_every_final_is_the_start(self):
        sources = finals(**{key: COMMITS[2] for key in LANES})
        self.assertEqual(plan_window(history(), COMMITS[6], sources),
                         (COMMITS[6], [key + ":before-adoption" for key in LANES]))
        sources["web"] = (COMMITS[6], "final")
        self.assertEqual(plan_window(history(), COMMITS[6], sources)[0], COMMITS[6])

    def test_an_unknown_adoption_is_left_to_the_existing_refusal(self):
        self.assertEqual(plan_window(history(), "f" * 40, finals(web=COMMITS[6])), ("f" * 40, []))

    def test_the_window_never_starts_before_adoption_or_after_any_lane_bound(self):
        chain, rng = history(12), random.Random(7)
        candidates = [None, "f" * 40, *COMMITS[:12]]
        for _ in range(500):
            adoption = rng.choice(COMMITS[:12])
            sources = {key: (source, "final") if source else (None, "no-final")
                       for key in LANES for source in [rng.choice(candidates)]}
            start, lanes = plan_window(chain, adoption, sources)
            floor = chain.position(adoption)
            bounds = [max(floor, chain.position(source)) if source in chain.positions else floor
                      for source, _reason in sources.values()]
            self.assertGreaterEqual(chain.position(start), floor)
            self.assertEqual(chain.position(start), min(bounds))
            self.assertTrue(lanes)

    def test_window_line_names_why_the_plan_covers_what_it_does(self):
        line = window_line({"start": COMMITS[4], "revisions": 6, "adoption": COMMITS[1], "bound_by": ["am", "eg"]})
        self.assertEqual(line, f"PLAN window start={COMMITS[4]} revisions=6 adoption={COMMITS[1]} bound-by=am,eg")
        line = window_line({"start": COMMITS[4], "revisions": 6, "adoption": COMMITS[1], "bound_by": ["am"],
                            "transition": COMMITS[2]})
        self.assertTrue(line.endswith(f" bound-by=am transition={COMMITS[2]}"))


class Repository:
    """A real first-parent chain; the shared selector is replaced by fixed decisions."""

    def __init__(self, test, count, *, contract=False):
        directory = tempfile.TemporaryDirectory()
        test.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.run("init", "--quiet")
        self.commits = []
        if contract:
            fixture_source(self.root)
            current = {path: (self.root / path).read_text() for path in (RECIPE, VERSION_FILE)}
            for path, content in current.items():
                (self.root / path).write_text(content.replace('"3"', '"2"'))
            self.run("add", ".")
            self.commit("Old contract")
            for path, content in current.items():
                (self.root / path).write_text(content)
            self.run("add", ".")
            self.commit("Transition")
        while len(self.commits) < count:
            self.commit(f"Merge {len(self.commits)}")
        self.decisions = {}

    def run(self, *args):
        return subprocess.run(["git", "-c", "user.name=Test", "-c", "user.email=test@example.invalid",
                               "-c", "commit.gpgSign=false", *args], cwd=self.root, check=True,
                              capture_output=True, text=True, timeout=10).stdout.strip()

    def commit(self, message):
        self.run("commit", "--allow-empty", "-qm", message)
        self.commits.append(self.run("rev-parse", "HEAD"))

    def tree(self, commit):
        return self.run("rev-parse", commit + "^{tree}")

    def batch(self, _root, revisions, _metrics, _budget):
        return {revision: {"deployables": self.decisions.get(revision, [])} for revision in revisions}

    def row(self, commit):
        return {"source": commit, "tree": self.tree(commit), "deployables": self.decisions.get(commit, [])}

    def plan(self, adoption, *, finals=None, selector=None):
        found = {}
        with patch.object(queue, "batch_selection", side_effect=self.batch) as batch:
            kwargs = {} if selector is None else {"anchor_selector": selector}
            result = release_queue(self.root, adoption, self.commits[-1], self.commits[-1], {},
                                   finals=finals, window=found, **kwargs)
        return result, [call.args[1] for call in batch.call_args_list], found


class ReleaseQueueWindowTests(unittest.TestCase):
    def test_without_finals_the_plan_is_unchanged(self):
        repo = Repository(self, 6)
        repo.decisions = {repo.commits[2]: ["cma-web"], repo.commits[4]: ["cma-agent-manager"]}
        before, batches, found = repo.plan(repo.commits[1])
        self.assertEqual(batches, [repo.commits[1:]])
        self.assertEqual(found, {})
        after, batches, found = repo.plan(repo.commits[1], finals=lambda: dict.fromkeys(LANES, (None, "no-final")))
        self.assertEqual((after, batches), (before, [repo.commits[1:]]))
        self.assertEqual(before["changes"], [repo.row(repo.commits[2]), repo.row(repo.commits[4])])
        self.assertEqual(found, {"start": repo.commits[1], "revisions": 5, "adoption": repo.commits[1],
                                 "bound_by": [key + ":no-final" for key in LANES]})

    def test_bounded_plan_selects_only_the_window_and_lists_its_first_revision(self):
        repo = Repository(self, 7)
        repo.decisions = {repo.commits[2]: ["cma-web"], repo.commits[4]: ["cma-web"], repo.commits[5]: ["cma-web"]}
        bounds = finals(**dict({key: repo.commits[3] for key in LANES}, web=repo.commits[4]))
        result, batches, found = repo.plan(repo.commits[1], finals=lambda: bounds)
        self.assertEqual(batches, [repo.commits[3:]])
        # The window's first revision selects nothing now, yet allocation must see it.
        self.assertEqual(result["changes"], [repo.row(repo.commits[3]), repo.row(repo.commits[4]),
                                             repo.row(repo.commits[5])])
        self.assertEqual(result["changes"][0]["deployables"], [])
        self.assertEqual(found, {"start": repo.commits[3], "revisions": 4, "adoption": repo.commits[1],
                                 "bound_by": [key for key in LANES if key != "web"]})

    def test_the_finals_reader_runs_only_after_develop_history_is_bound(self):
        repo = Repository(self, 4)
        reader = Mock(side_effect=AssertionError)
        with self.assertRaisesRegex(Refusal, "no longer on develop"), patch.object(queue, "batch_selection"):
            release_queue(repo.root, repo.commits[0], repo.commits[-1], "f" * 40, {}, finals=reader)
        reader.assert_not_called()
        with self.assertRaisesRegex(Refusal, "Adoption commit"), patch.object(queue, "batch_selection"):
            release_queue(repo.root, "f" * 40, repo.commits[-1], repo.commits[-1], {},
                          finals=lambda: finals(**{key: repo.commits[2] for key in LANES}))

    def test_a_transition_older_than_every_final_stays_in_the_plan(self):
        repo = Repository(self, 6, contract=True)
        old, transition, *later = repo.commits
        repo.decisions = {transition: ALL_IMAGES, later[0]: GROUP_IMAGES,
                          later[1]: ["cma-event-gateway", "cma-web"], later[3]: ["cma-agent-manager"]}
        bounds = finals(**{key: later[0] for key in GROUP}, eg=later[1], web=later[1])
        full, batches, _found = repo.plan(old)
        self.assertEqual(full["contract_transition"], transition)
        selector = Mock(return_value=ALL_IMAGES)
        bounded, batches, found = repo.plan(old, finals=lambda: bounds, selector=selector)
        selector.assert_called_once_with(repo.root, transition)
        self.assertEqual(batches, [later])
        self.assertEqual(bounded, full)
        self.assertEqual(found["transition"], transition)
        self.assertEqual((found["start"], found["revisions"]), (later[0], 4))

    def test_a_transition_inside_the_window_needs_no_separate_selection(self):
        repo = Repository(self, 4, contract=True)
        old, transition, *later = repo.commits
        repo.decisions = {transition: GROUP_IMAGES, later[0]: ["cma-web"]}
        bounds = finals(**{key: transition for key in LANES})
        selector = Mock(side_effect=AssertionError)
        bounded, batches, found = repo.plan(old, finals=lambda: bounds, selector=selector)
        self.assertEqual(bounded["contract_transition"], transition)
        self.assertEqual(batches, [repo.commits[1:]])
        self.assertNotIn("transition", found)

    def test_an_unselected_earlier_transition_falls_back_to_the_adoption_window(self):
        repo = Repository(self, 5, contract=True)
        old, transition, *later = repo.commits
        repo.decisions = {transition: GROUP_IMAGES, later[1]: ["cma-web"]}
        bounds = finals(**{key: later[0] for key in LANES})
        full, _batches, _found = repo.plan(old)
        bounded, batches, found = repo.plan(old, finals=lambda: bounds, selector=Mock(return_value=["cma-web"]))
        self.assertEqual(bounded, full)
        self.assertEqual(batches, [later, repo.commits])
        self.assertEqual((found["start"], found["bound_by"]), (old, ["transition:unselected"]))

    def test_an_unreadable_contract_history_falls_back_to_the_adoption_window(self):
        repo = Repository(self, 5, contract=True)
        old, transition, *later = repo.commits
        repo.decisions = {later[1]: ["cma-web"]}
        bounds = finals(**{key: later[0] for key in LANES})
        full, _batches, _found = repo.plan(old)
        with patch.object(window.ContractPolicy, "transition", side_effect=Refusal("Runtime contract source is unavailable")):
            bounded, batches, found = repo.plan(old, finals=lambda: bounds)
        self.assertEqual(bounded, full)
        self.assertEqual(batches, [later, repo.commits])
        self.assertEqual((found["start"], found["bound_by"]), (old, ["transition:unreadable"]))


def bounded_plan(develop):
    """Plan a push the way staging_cd_queue does, from the fixture's newest finals."""
    def plan(full):
        chain = develop.server.history()
        commits = sorted(chain.positions, key=chain.position, reverse=True)
        decisions = {change["source"]: change["deployables"] for change in full["changes"]}
        sources = {key: (receipt["source"], "final") for key, receipt in develop.finals.items()}
        adoption = develop.source(0)
        start, _lanes = plan_window(chain, adoption, sources)

        def command(_root, *args):
            return "\n".join(commits) if args[0] == "rev-list" else "e" * 40
        rows = reconcile(None, start, full["head"], lambda _root, revision: decisions.get(revision, []),
                         command, keep_first=develop.keep_first and start != adoption)
        develop.windows.append((start, len(rows), len(full["changes"])))
        return dict(full, changes=rows)
    return plan


class AllocationEquivalenceTests(unittest.TestCase):
    """Allocating a bounded plan reserves and resumes exactly what the adoption window would."""

    def lockstep(self, *, keep_first=True):
        full, bounded = Develop(10), Develop(10)
        bounded.keep_first, bounded.windows = keep_first, []
        bounded.plan = bounded_plan(bounded)
        return full, bounded

    def push(self, pair, components, **kwargs):
        full, bounded = pair
        expected, actual = full.push(components, **kwargs), bounded.push(components, **kwargs)
        self.assertEqual(actual, expected)
        return expected

    def finalize(self, pair, tasks, *components):
        for develop in pair:
            for component in components:
                develop.finalize(tasks[component])

    def complete(self, pair, run_id):
        for develop in pair:
            develop.complete(run_id)

    def test_bounded_plans_allocate_like_the_adoption_window(self):
        pair = self.lockstep()
        self.assertEqual(self.push(pair, set()), {})
        self.complete(pair, 801)
        self.finalize(pair, self.push(pair, {"web"}), "web")
        self.complete(pair, 802)
        self.finalize(pair, self.push(pair, {"eg"}), "eg")
        self.complete(pair, 803)
        # Until every lane has a final, the adoption commit bounds the plan.
        self.finalize(pair, self.push(pair, set(LANES)), *LANES)
        self.complete(pair, 804)
        web = self.push(pair, {"web"})
        self.assertEqual(web.keys(), set(LANES))  # The others resume their unproven finals.
        self.finalize(pair, web, "web")
        self.complete(pair, 805)
        # The gateway build fails; a later publisher-only merge retries it at its source.
        self.assertEqual(self.push(pair, {"eg"})["eg"]["mode"], "publish")
        self.complete(pair, 806)
        retry = self.push(pair, set(), verified={"web", "am", "runtime", "runtime-omarchy"})
        self.assertEqual((retry.keys(), retry["eg"]["reservation"]["source"]), ({"eg"}, pair[0].source(5)))
        self.finalize(pair, retry, "eg")
        self.complete(pair, 807)
        # A narrower selector no longer selects web's newest final.
        for develop in pair:
            develop.unselect(4)
        agent = self.push(pair, {"am"}, verified={"eg"})
        self.assertEqual(agent.keys(), set(LANES) - {"eg"})
        self.finalize(pair, agent, "am")
        self.complete(pair, 808)
        runtimes = self.push(pair, {"runtime", "runtime-omarchy"}, verified={"eg", "am", "web"})
        self.finalize(pair, runtimes, "runtime", "runtime-omarchy")
        self.complete(pair, 809)
        # Web's unselected newest final now bounds the window and still resumes.
        resumed = self.push(pair, set(), verified={"eg", "am", "runtime", "runtime-omarchy"})
        self.assertEqual(resumed, {"web": {"component": "web", "mode": "resume", "receipt": pair[0].finals["web"]}})
        self.complete(pair, 810)
        self.assertEqual(self.push(pair, set(), verified=set(LANES)), {})
        full, bounded = pair
        self.assertEqual(bounded.server.refs, full.server.refs)
        starts = [start for start, _rows, _total in bounded.windows]
        self.assertEqual(starts, [full.source(0)] * 4 + [full.source(3)] * 5 + [full.source(4)] * 2)
        rows, total = bounded.windows[-1][1:]
        self.assertEqual((rows, total), (4, 6))
        self.assertEqual(bounded.queues[811]["changes"][0], {"source": full.source(4), "tree": "e" * 40,
                                                             "deployables": []})

    def test_an_exclusive_start_would_strand_an_unselected_newest_final(self):
        # Why the oldest newest-final source stays inside the window.
        for keep_first, expected in ((True, {"web"}), (False, set())):
            with self.subTest(keep_first=keep_first):
                full, bounded = self.lockstep(keep_first=keep_first)
                for develop in (full, bounded):
                    develop.push(set())
                    develop.complete(801)
                    tasks = develop.push(set(LANES))
                    for key in LANES:
                        develop.finalize(tasks[key])
                    develop.complete(802)
                    develop.finalize(develop.push({"web"})["web"])
                    develop.complete(803)
                    develop.unselect(2)
                    for key in sorted(set(LANES) - {"web"}):
                        develop.finalize(develop.push({key})[key])
                        develop.complete(801 + len(develop.runs) - 1)
                verified = set(LANES) - {"web"}
                self.assertEqual(full.push(set(), verified=verified).keys(), {"web"})
                self.assertEqual(bounded.push(set(), verified=verified).keys(), expected)


class MainWindowTests(unittest.TestCase):
    def test_main_reads_finals_lazily_and_prints_the_chosen_window(self):
        with tempfile.TemporaryDirectory() as directory:
            outputs = Path(directory) / "outputs"
            outputs.write_text("")
            env = {"GITHUB_OUTPUT": str(outputs), "GITHUB_EVENT_NAME": "push", "GITHUB_SHA": COMMITS[3],
                   "GH_TOKEN": "synthetic-not-a-credential", "CD_ADOPTION_SHA": COMMITS[0]}

            def plan(root, adoption, head, event_source, metrics, *, budget, finals, window):
                self.assertEqual((adoption, head, event_source, budget), (COMMITS[0], COMMITS[3], COMMITS[3],
                                                                         queue.BUDGET_SECONDS))
                self.assertEqual(finals(), "read")
                window.update(start=COMMITS[2], revisions=2, adoption=adoption, bound_by=["eg"])
                return {"head": head, "event_source": event_source, "changes": []}
            stdout = io.StringIO()
            with patch.dict(os.environ, env), patch.object(queue, "event_identity"), \
                    patch.object(queue, "git", return_value=COMMITS[3]), \
                    patch.object(queue, "read_finals", return_value="read") as reader, \
                    patch.object(queue, "release_queue", side_effect=plan), contextlib.redirect_stdout(stdout):
                self.assertEqual(queue.main([]), 0)
            reader.assert_called_once_with(Path.cwd(), "synthetic-not-a-credential")
            self.assertIn(f"PLAN window start={COMMITS[2]} revisions=2 adoption={COMMITS[0]} bound-by=eg\n",
                          stdout.getvalue())


if __name__ == "__main__":
    unittest.main()
