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
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch

import ci_retry
from ci_tracks.git_comparison import comparison_details, first_parents, shallow
from staging_cd_behind_fixture import Develop
from staging_cd_common import Refusal
from staging_cd_compatibility import GROUP, RECIPE, VERSION_FILE
from staging_cd_contract_tests import fixture_source
from staging_cd_history import DevelopHistory
from staging_cd_plan_window import (MANIFEST, PROPAGATION_SECONDS, PlanRefused, Window, newest_finals, plan_window,
                                    read_finals, window_line, window_warning)
import staging_cd_plan_window as window
import staging_cd_queue as queue
from staging_cd_queue import release_queue
from staging_cd_receipts import annotation, final_tag_oid, final_tag_text, git_tag_oid, parse_annotation
from staging_cd_receipts_tests import fixture_receipt, fixture_runtime_receipt
from staging_cd_release_tags import decode_tag
from staging_cd_versions import DEPLOYABLES

COMMITS = [f"{index:040x}" for index in range(1, 21)]
LANES = sorted(DEPLOYABLES)
GROUP_IMAGES = sorted(DEPLOYABLES[key] for key in GROUP)
ALL_IMAGES = sorted(DEPLOYABLES.values())
# Seeds of the randomized lockstep; CD_PLAN_LOCKSTEP_SEEDS runs more locally.
SEEDS = int(os.environ.get("CD_PLAN_LOCKSTEP_SEEDS", "3"))
# A real final tag object as published (cma-web/v0.17.6), and its ID.
REAL_TAG_OID = "b7b302a23c9978964d8091a532aef6ba4c597eee"
REAL_TAG = (
    b"object 45423f579b0115bd3bb0d075526597d86bcc32ca\ntype commit\ntag cma-web/v0.17.6\n"
    b"tagger github-actions[bot] <41898282+github-actions[bot]@users.noreply.github.com> 0 +0000\n\n"
    b'{"archive":"7bd5652e7da7678e652381c6d512950fabb7e94dbba7e0e9dc8cee6a4696bc61",'
    b'"digest":"sha256:60607c565fea91dc80c2856580f76386a7fa4e76661f1eced7397d5f1c6b7c6c",'
    b'"identity":{"build":"47fdf94bd9ef8076d1dbecd5187e5607403c9470894eda2e0b92129af54a8ea3","bump":"patch",'
    b'"component":"web","pr":2909,"previous":"0.17.5","run_attempt":1,"run_id":44914556335,'
    b'"schema":"cma-image-release-v1","source":"45423f579b0115bd3bb0d075526597d86bcc32ca",'
    b'"tree":"041eb78fb3eaf7f5dd68a6d0e5adc76e0a4527d0","version":"0.17.6"},'
    b'"repository":"ghcr.io/chronoaiproject/cma-web","scan":{"bundle":'
    b'"a3eb9041f1b920bda1f8b2bd63241ea5ed0c890f60841a7303670b35dbb384c3","complete":true,"counts":'
    b'{"advisory":2765,"allowlisted":0,"blocking":0,"detected":2779,"reviewed":14},"mode":"credential",'
    b'"version":"1.0.4"},"schema":"cma-published-image-v1","source":"45423f579b0115bd3bb0d075526597d86bcc32ca",'
    b'"tag":"cma-web/v0.17.6"}\n')


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


def failure(status=None, headers=None, transport=False):
    """A failed read as staging_cd_github raises it."""
    error = Refusal("GitHub API request failed")
    cause = ci_retry.TransientError() if transport else None
    if status is not None:
        error.status = status
        cause = ci_retry.status_cause(status, headers)
    error.__cause__ = cause
    return error


class Tags:
    """A live advertisement and the tag objects GitHub serves for it, with scripted failures."""

    def __init__(self, sources=(), develop=COMMITS[9]):
        self.refs, self.objects, self.failures, self.reads = {"refs/heads/develop": develop}, {}, {}, []
        for component, versions in dict(sources).items():
            for version, source in versions.items():
                self.add(*final(component, source, version))

    def add(self, ref, oid, tag):
        self.refs[ref] = oid
        self.objects[oid] = tag

    def request(self, path, **_kwargs):
        oid = path.removeprefix("git/tags/")
        self.reads.append(oid)
        if self.failures.get(oid):
            raise self.failures[oid].pop(0)
        if oid not in self.objects:
            raise failure(transport=True)
        return copy.deepcopy(self.objects[oid])

    def advertisement(self):
        return "".join(oid + "\t" + ref + "\n" for ref, oid in self.refs.items())


class Clock:
    def __init__(self):
        self.now, self.waits = 0.0, []

    def __call__(self):
        return self.now

    def sleep(self, seconds):
        self.waits.append(seconds)
        self.now += seconds


class NewestFinalTests(unittest.TestCase):
    def tags(self):
        # Each lane has an older and a newer final; only the newer one bounds it.
        return Tags({key: {"0.10.1": COMMITS[1], "0.10.2": COMMITS[3 + index]} for index, key in enumerate(LANES)})

    def read(self, tags, clock=None):
        clock = clock or Clock()
        return newest_finals(tags, tags.refs, deadline=clock() + 240, clock=clock, sleep=clock.sleep)

    def test_each_lane_is_bounded_by_its_newest_verified_final(self):
        tags = self.tags()
        self.assertEqual(self.read(tags), {key: (COMMITS[3 + index], "final") for index, key in enumerate(LANES)})

    def test_versions_order_finals_not_their_creation_or_name_order(self):
        tags = Tags({"web": {"0.10.9": COMMITS[2], "0.10.10": COMMITS[5]}})
        self.assertEqual(self.read(tags)["web"], (COMMITS[5], "final"))

    def test_lane_without_a_final_or_with_only_reservations_is_new(self):
        tags = Tags({"web": {"0.10.1": COMMITS[2]}})
        tags.refs["refs/tags/cma-event-gateway/pending/v0.10.1"] = "a" * 40
        self.assertEqual(self.read(tags), dict(dict.fromkeys(LANES, (None, "new")), web=(COMMITS[2], "final")))

    def test_malformed_version_ref_makes_every_lane_untrusted(self):
        for ref in ("refs/tags/cma-web/v1.2", "refs/tags/cma-web/v01.2.3", "refs/tags/cma-runtime/vnext"):
            with self.subTest(ref=ref):
                tags = self.tags()
                tags.refs[ref] = "b" * 40
                self.assertEqual(self.read(tags), dict.fromkeys(LANES, (None, "untrusted")))

    def test_the_plan_accepts_exactly_the_finals_allocation_accepts(self):
        ref, oid, tag = final("web", COMMITS[6], "0.10.3")
        receipt = json.loads(tag["message"])
        other = fixture_receipt(component="web", source=COMMITS[7], version="0.10.3", previous="0.10.2")
        cases = {
            "object for another ref": (dict(tag, tag="cma-web/v0.10.2"), (None, "untrusted")),
            "lightweight target": (dict(tag, object={"type": "tree", "sha": COMMITS[6]}), (None, "untrusted")),
            "receipt for another commit": (dict(tag, object={"type": "commit", "sha": COMMITS[7]}), (None, "untrusted")),
            "not a receipt": (dict(tag, message="release"), (None, "untrusted")),
            # Not byte for byte the canonical tag of its receipt (a tagger or format
            # change), yet consistent: allocation accepts it, so the plan does too.
            "another writer's format": (dict(tag, message=annotation(other), object={"type": "commit", "sha": COMMITS[7]}),
                                        (COMMITS[7], "final")),
            "altered receipt": (dict(tag, message=annotation(dict(receipt, digest="sha256:" + "0" * 64))),
                                (COMMITS[6], "final")),
        }
        for name, (served, expected) in cases.items():
            with self.subTest(name):
                tags = self.tags()
                tags.add(ref, oid, served)
                result = self.read(tags)
                self.assertEqual(result["web"], expected)
                self.assertEqual(result["eg"], (COMMITS[4], "final"))

    def test_the_release_tag_format_is_pinned_to_a_real_published_final(self):
        # Finalization writes final_tag_text and recovers by its ID; a change of
        # tagger or encoding would show here first.
        self.assertEqual(git_tag_oid(REAL_TAG), REAL_TAG_OID)
        tag = decode_tag(REAL_TAG_OID, REAL_TAG)
        receipt = parse_annotation(tag["message"])
        self.assertEqual(final_tag_text(receipt).encode(), REAL_TAG)
        self.assertEqual(final_tag_oid(receipt), REAL_TAG_OID)
        tags = Tags()
        tags.add("refs/tags/cma-web/v0.17.6", REAL_TAG_OID, tag)
        self.assertEqual(self.read(tags)["web"], ("45423f579b0115bd3bb0d075526597d86bcc32ca", "final"))

    def test_transient_reads_repeat_and_a_just_advertised_404_is_propagation(self):
        oid = self.tags().refs["refs/tags/cma-web/v0.10.2"]
        for name, errors in (("server errors", [failure(502), failure(503)]),
                             ("transport", [failure(transport=True)] * 3),
                             ("not yet propagated", [failure(404), failure(404)])):
            with self.subTest(name):
                tags, clock = self.tags(), Clock()
                tags.failures[oid] = list(errors)
                self.assertEqual(self.read(tags, clock)["web"], (COMMITS[7], "final"))
                self.assertEqual(len(clock.waits), len(errors))
                self.assertEqual(tags.reads.count(oid), len(errors) + 2)  # Then served from the reader's copy.

    def test_a_throttled_read_waits_as_long_as_github_asks(self):
        oid = self.tags().refs["refs/tags/cma-web/v0.10.2"]
        tags, clock = self.tags(), Clock()
        tags.failures[oid] = [failure(429, {"retry-after": "45"})]
        self.assertEqual(self.read(tags, clock)["web"], (COMMITS[7], "final"))
        self.assertEqual(len(clock.waits), 1)
        self.assertGreaterEqual(clock.waits[0], 45)

    def test_only_a_rejected_credential_is_denied(self):
        oid = self.tags().refs["refs/tags/cma-web/v0.10.2"]
        invalid = Refusal("GitHub API response is invalid")  # No status, nothing transient: a body that did not parse.
        for name, errors, kind, retried in (
                ("server errors", [failure(503)] * 6, "unreadable", True),
                ("a secondary rate limit without its headers", [failure(403)] * 6, "unreadable", True),
                ("an exhausted quota", [failure(403, {"x-ratelimit-remaining": "0"})] * 6, "unreadable", True),
                ("an invalid body", [invalid], "unreadable", False),
                ("a rejected credential", [failure(401)], "denied", False)):
            with self.subTest(name):
                tags, clock = self.tags(), Clock()
                tags.failures[oid] = list(errors)
                self.assertEqual(self.read(tags, clock)["web"], (None, kind))
                self.assertEqual(bool(clock.waits), retried)
                self.assertLessEqual(clock.now, 240)
        # A requested wait past the deadline ends the read instead of sleeping; one wait is capped.
        tags, clock = self.tags(), Clock()
        tags.failures[oid] = [failure(503, {"retry-after": "600"})] * 6
        self.assertEqual(newest_finals(tags, tags.refs, deadline=50, clock=clock, sleep=clock.sleep)["web"],
                         (None, "unreadable"))
        self.assertEqual(clock.waits, [])
        tags, clock = self.tags(), Clock()
        tags.failures[oid] = [failure(503, {"retry-after": "600"})]
        self.assertEqual(self.read(tags, clock)["web"], (COMMITS[7], "final"))
        self.assertEqual(clock.waits, [ci_retry.RETRY_AFTER_CAP])

    def test_only_a_404_that_outlasts_its_propagation_window_is_untrusted(self):
        # Full-jitter waits are the draw times 2, 4, 8, 16 and 30 seconds.
        oid = self.tags().refs["refs/tags/cma-web/v0.10.2"]
        for name, draw, policy, deadline, kind, ended in (
                # The fifth 404 arrives exactly when the window closes: the final is not served.
                ("window closed", 1.0, {"attempts": 5}, 240, "untrusted", 30),
                # Attempts run out 1.5 s before the window closes: an outage catch-up retries.
                ("attempts spent just short", 0.95, {"attempts": 5}, 240, "unreadable", 28.5),
                # The window closes between the fifth and sixth 404s.
                ("window closed during the wait", 0.95, {}, 240, "untrusted", 57),
                # The budget ends 6 s in, before the next wait could finish.
                ("budget spent", 1.0, {}, 10, "unreadable", 6)):
            with self.subTest(name), patch.dict(window.READ_POLICY, rand=lambda draw=draw: draw, **policy):
                tags, clock = self.tags(), Clock()
                tags.failures[oid] = [failure(404)] * 50
                result = newest_finals(tags, tags.refs, deadline=deadline, clock=clock, sleep=clock.sleep)
                self.assertEqual(result["web"], (None, kind))
                self.assertAlmostEqual(clock.now, ended)

    def test_any_failed_listing_repeats_and_only_a_malformed_one_is_untrusted(self):
        chain, tags = history(), Tags()  # No finals: only the listing is read.
        listed = subprocess.CompletedProcess([], 0, tags.advertisement(), "")
        diagnostics = [
            "fatal: unable to access: Failed to connect to github.com port 443 after 3 ms: Couldn't connect to server",
            "fatal: unable to access: Empty reply from server",
            "error: RPC failed; curl 92 HTTP/2 stream 1 was not closed cleanly: CANCEL (err 8)",
            "fatal: expected flush after ref listing",
            "fatal: repository not found",
            "fatal: unable to access: Could not resolve host: github.com",
        ]
        for diagnostic in diagnostics:
            for persistent in (False, True):
                with self.subTest(diagnostic=diagnostic, persistent=persistent):
                    clock = Clock()
                    failed = subprocess.CompletedProcess([], 128, "", diagnostic)
                    runner = Mock(side_effect=[failed] * (6 if persistent else 2) + [listed] * 3)
                    develop, lanes = read_finals(Path.cwd(), "fixture", chain, runner=runner, clock=clock,
                                                 sleep=clock.sleep)
                    if persistent:
                        self.assertEqual((develop, lanes), (None, dict.fromkeys(LANES, (None, "unreadable"))))
                    else:
                        self.assertEqual((develop, lanes, len(clock.waits)),
                                         (COMMITS[9], dict.fromkeys(LANES, (None, "new")), 2))
        for responses, kind in (([subprocess.TimeoutExpired(["git"], 60)] * 6, "unreadable"),
                                ([OSError("git is missing")] * 6, "unreadable"),
                                ([subprocess.CompletedProcess([], 0, "not an advertisement", "")], "untrusted"),
                                ([subprocess.CompletedProcess([], 0, "a" * 40 + "\trefs/tags/x\n", "")], "untrusted")):
            with self.subTest(kind=kind, first=str(responses[0])[:60]):
                clock = Clock()
                develop, lanes = read_finals(Path.cwd(), "fixture", chain, runner=Mock(side_effect=responses),
                                             clock=clock, sleep=clock.sleep)
                self.assertEqual((develop, lanes), (None, dict.fromkeys(LANES, (None, kind))))

    def test_reader_sees_credentials_and_stale_runs(self):
        chain = history()
        listed = Mock(return_value=subprocess.CompletedProcess([], 0, Tags().advertisement(), ""))
        for token in (None, "", "two words"):
            with self.subTest(token=token):
                self.assertEqual(read_finals(Path.cwd(), token, chain, runner=listed),
                                 (COMMITS[9], dict.fromkeys(LANES, (None, "denied"))))
        # Only a verified first-parent advance lets planning skip the event.
        listed = Mock(return_value=subprocess.CompletedProcess([], 0, Tags(develop=COMMITS[15]).advertisement(), ""))
        with patch.object(DevelopHistory, "from_head", return_value=history(16)), \
                patch("staging_cd_plan_window.GitHub", side_effect=AssertionError):
            self.assertEqual(read_finals(Path.cwd(), "fixture", chain, runner=listed), (COMMITS[15], None))
        clock = Clock()
        with patch.object(DevelopHistory, "from_head", return_value=DevelopHistory(COMMITS[15], [COMMITS[15]])):
            self.assertEqual(read_finals(Path.cwd(), "fixture", chain, runner=listed, clock=clock, sleep=clock.sleep),
                             (None, dict.fromkeys(LANES, (None, "unreadable"))))
        self.assertTrue(clock.waits)

    def test_lagging_advertisement_is_retried_before_reading_finals(self):
        older, current = (subprocess.CompletedProcess([], 0, Tags(develop=head).advertisement(), "")
                          for head in (COMMITS[7], COMMITS[9]))
        clock = Clock()
        runner = Mock(side_effect=[older, current])
        self.assertEqual(read_finals(Path.cwd(), "fixture", history(), runner=runner, clock=clock, sleep=clock.sleep),
                         (COMMITS[9], dict.fromkeys(LANES, (None, "new"))))
        self.assertEqual((runner.call_count, len(clock.waits)), (2, 1))


# Every lane is in the manifest from the start of these histories.
ADDED = dict.fromkeys(LANES, COMMITS[0])


class PlanWindowTests(unittest.TestCase):
    def plan(self, adoption, sources, introduced=None, count=10):
        return plan_window(history(count), adoption, sources, dict(ADDED, **(introduced or {})))

    def refused(self, *args, **kwargs):
        with self.assertRaises(PlanRefused) as refused:
            self.plan(*args, **kwargs)
        return refused.exception.kind, refused.exception.lanes, refused.exception.at

    def test_window_starts_at_the_oldest_lane_newest_final(self):
        sources = finals(**{"am": COMMITS[4], "eg": COMMITS[6], "runtime": COMMITS[4],
                            "runtime-omarchy": COMMITS[5], "web": COMMITS[8]})
        self.assertEqual(self.plan(COMMITS[1], sources), Window(COMMITS[4], ["am", "runtime"], COMMITS[1]))

    def test_the_adoption_commit_is_not_needed_once_every_lane_has_a_final(self):
        sources = finals(**dict({key: COMMITS[6] for key in LANES}, web=COMMITS[3]))
        for adoption in (None, "", "f" * 40):  # Unset, or older than this history.
            with self.subTest(adoption=adoption):
                self.assertEqual(self.plan(adoption, sources).start, COMMITS[3])

    def test_an_adoption_newer_than_the_oldest_final_is_still_the_start(self):
        sources = finals(**{key: COMMITS[2] for key in LANES})
        self.assertEqual(self.plan(COMMITS[6], sources), Window(COMMITS[6], ["adoption"], COMMITS[1]))
        self.assertEqual(self.plan(COMMITS[2], sources).bound_by, LANES)

    def test_a_lane_that_never_had_a_final_starts_after_the_merge_that_added_it(self):
        # The selector cannot compare the addition itself: its parent lacks the lane.
        sources = finals(**{key: COMMITS[6] for key in LANES if key != "web"})
        self.assertEqual(self.plan(None, sources, {"web": COMMITS[3]}),
                         Window(COMMITS[4], ["web:new"], COMMITS[4]))
        # A window that would reach any lane's addition refuses at once, naming it.
        older = dict(sources, eg=(COMMITS[2], "final"))
        self.assertEqual(self.refused(None, older, {"web": COMMITS[3]}), ("introduced", ["web"], COMMITS[3]))
        # Setting the adoption commit after the addition lets the plan go on.
        self.assertEqual(self.plan(COMMITS[4], older, {"web": COMMITS[3]}).start, COMMITS[4])
        # The push that adds a lane cannot be planned until a merge follows it.
        self.assertEqual(self.refused(None, sources, {"web": COMMITS[9]}), ("introduced", ["web"], COMMITS[9]))

    def test_only_a_lane_without_a_bound_on_this_history_needs_the_adoption_floor(self):
        sources = finals(**{key: COMMITS[6] for key in LANES})
        for name, lane, introduced in (("final off history", ("f" * 40, "final"), {}),
                                       ("added before this history", (None, "new"), {"web": None})):
            with self.subTest(name):
                bounded = dict(sources, web=lane)
                window = self.plan(COMMITS[2], bounded, introduced)
                self.assertEqual((window.start, window.bound_by), (COMMITS[2], ["web:floor"]))
                for adoption in (None, "f" * 40):
                    self.assertEqual(self.refused(adoption, bounded, introduced), ("floor", ["web"], None))

    def test_a_lane_that_cannot_be_read_or_trusted_refuses_the_plan_at_once(self):
        sources = finals(**{key: COMMITS[6] for key in LANES})
        for kind in ("unreadable", "denied", "untrusted"):
            self.assertEqual(self.refused(COMMITS[1], dict(sources, eg=(None, kind))), (kind, ["eg"], None))
        # Most actionable first; only an outage alone is the transient stop catch-up re-runs.
        mixed = dict(sources, eg=(None, "unreadable"), web=(None, "untrusted"), am=(None, "denied"))
        self.assertEqual(self.refused(COMMITS[1], mixed), ("untrusted", ["web"], None))
        self.assertEqual(self.refused(COMMITS[1], {}), ("unreadable", LANES, None))

    def test_the_window_never_starts_before_adoption_or_after_any_lane_bound(self):
        chain, rng = history(12), random.Random(7)
        candidates = [None, "f" * 40, *COMMITS[:12]]
        added = chain.position(COMMITS[0])
        for _ in range(500):
            adoption = rng.choice([None, "f" * 40, *COMMITS[:12]])
            sources = {key: (rng.choice(candidates), "final") for key in LANES}
            floor = chain.positions.get(adoption)
            bounds = [chain.positions.get(source, floor) for source, _kind in sources.values()]
            expected = None if None in bounds else max(min(bounds), floor if floor is not None else min(bounds))
            try:
                window = plan_window(chain, adoption, sources, dict.fromkeys(LANES, COMMITS[0]))
            except PlanRefused as refused:
                if refused.kind == "floor":
                    self.assertIsNone(floor)
                    self.assertIsNone(expected)
                else:  # Every lane was added at the first commit; no window may reach it.
                    self.assertEqual(refused.kind, "introduced")
                    self.assertLessEqual(expected, added)
                continue
            self.assertEqual(chain.position(window.start), expected)
            self.assertGreater(expected, added)
            self.assertTrue(window.bound_by)

    def test_window_line_names_why_the_plan_covers_what_it_does(self):
        line = window_line({"start": COMMITS[4], "revisions": 6, "adoption": COMMITS[1], "bound_by": ["am", "eg"]})
        self.assertEqual(line, f"PLAN window start={COMMITS[4]} revisions=6 adoption={COMMITS[1]} bound-by=am,eg")
        line = window_line({"start": COMMITS[4], "revisions": 6, "adoption": "none", "bound_by": ["am"],
                            "transition": COMMITS[2]})
        self.assertEqual(line, f"PLAN window start={COMMITS[4]} revisions=6 adoption=none bound-by=am "
                               f"transition={COMMITS[2]}")
        self.assertEqual(window_line({"start": COMMITS[4], "refused": "floor", "lanes": ["eg", "web"], "at": None}),
                         "PLAN refused reason=floor lanes=eg,web")
        self.assertEqual(window_line({"refused": "introduced", "lanes": ["web"], "at": COMMITS[3]}),
                         f"PLAN refused reason=introduced lanes=web at={COMMITS[3]}")
        self.assertEqual(window_line({"stale": COMMITS[9], "head": COMMITS[4]}),
                         f"PLAN stale develop={COMMITS[9]} head={COMMITS[4]}")

    def test_a_wide_or_deep_window_warns_and_names_the_lever(self):
        def warning(revisions):
            return window_warning({"start": COMMITS[4], "revisions": revisions, "adoption": "none",
                                   "bound_by": ["eg"]})
        self.assertIsNone(warning(150))
        wide = warning(151)
        self.assertTrue(wide.startswith("::warning title=Wide release plan::The plan reconciles 151 "))
        self.assertIn("CMA_CD_ADOPTION_SHA", wide)
        deep = warning(1801)
        self.assertTrue(deep.startswith("::warning title=Release plan near its history limit::"))
        self.assertIn("CMA_CD_ADOPTION_SHA", deep)


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

    def test_a_new_lane_starts_after_the_merge_that_added_it(self):
        repo = Repository(self, 6, added={2: ["cma-web"]})
        repo.decisions = {repo.commits[4]: ["cma-agent-manager"]}
        lanes = finals(**{key: repo.commits[4] for key in LANES if key != "web"})
        result, found = repo.plan(None, lanes=lanes)
        self.assertEqual(windows(repo.calls), [repo.commits[3:]])
        self.assertEqual((found["start"], found["bound_by"]), (repo.commits[3], ["web:new"]))
        self.assertEqual(result["changes"], [repo.row(repo.commits[3]), repo.row(repo.commits[4])])
        # A window reaching the addition refuses before any selection, naming the merge.
        repo.calls.clear()
        older = finals(**{key: repo.commits[1] for key in LANES if key != "web"})
        with self.assertRaises(PlanRefused) as refused:
            repo.plan(None, lanes=older)
        self.assertEqual((refused.exception.kind, refused.exception.lanes, refused.exception.at, repo.calls),
                         ("introduced", ["web"], repo.commits[2], []))
        # An adoption commit after the addition is the operator's way through.
        _result, found = repo.plan(repo.commits[3], lanes=older)
        self.assertEqual((found["start"], found["bound_by"]), (repo.commits[3], ["adoption"]))
        # A lane added before this history needs the floor.
        with patch.object(queue, "introductions", return_value=dict.fromkeys(LANES)), \
                self.assertRaises(PlanRefused) as refused:
            repo.plan(None, lanes=lanes)
        self.assertEqual((refused.exception.kind, refused.exception.lanes), ("floor", ["web"]))

    def test_the_real_selector_cannot_compare_across_a_lane_addition(self):
        # Why the window may never reach a lane's addition: the selector's own
        # manifest reader refuses the parent snapshot, which lacks the lane.
        manifest = json.loads((Path(queue.__file__).parent / "deployables.json").read_text())
        script = ("import json, sys\nfrom pathlib import Path\nsys.path.insert(0, sys.argv[1])\n"
                  "from image_inputs import recipes\nfrom uncertainty import SelectionIntegrityError\n"
                  "try:\n    print(json.dumps(sorted(recipes(Path(sys.argv[2])))))\n"
                  "except SelectionIntegrityError:\n    print('refused')\n")
        for names, expected in ((sorted(manifest), json.dumps(sorted(manifest))),
                                (sorted(set(manifest) - {"cma-web"}), "refused")):
            with self.subTest(names=len(names)), tempfile.TemporaryDirectory() as directory:
                tree = Path(directory)
                (tree / MANIFEST).parent.mkdir(parents=True)
                (tree / MANIFEST).write_text(json.dumps({name: manifest[name] for name in names}))
                # This manifest check also runs in the parser-free staging CI environment.
                result = subprocess.run([sys.executable, "-S", "-B", "-c", script, str(Path(queue.__file__).parent / "ci_tracks"),
                                         str(tree)], capture_output=True, text=True, timeout=60, check=True)
                self.assertEqual(result.stdout.strip(), expected)

    def test_a_transition_before_a_lane_addition_is_never_an_anchor(self):
        # It can no longer be compared, and the adoption window could not reach it either.
        repo = Repository(self, 7, contract_at=1, added={3: ["cma-web"]})
        repo.decisions = {repo.commits[1]: GROUP_IMAGES, repo.commits[5]: ALL_IMAGES}
        result, found = repo.plan(repo.commits[0], lanes=finals(**{key: repo.commits[5] for key in LANES}))
        self.assertEqual(windows(repo.calls, cached=False), [])
        self.assertNotIn("contract_transition", result)
        self.assertNotIn("transition", found)

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
            self.assertEqual(first_parents(clone, [anchor]), {anchor: repo.commits[1]})
            with self.assertRaises(ValueError):
                first_parents(clone, [repo.commits[0]])
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
    def run_main(self, plan, argv=(), extra=None, unset=()):
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
                for key in unset:
                    os.environ.pop(key, None)
                status = queue.main(list(argv))
            return status, outputs.read_text(), stdout.getvalue(), reader

    def test_main_reads_finals_lazily_and_prints_the_chosen_window(self):
        def plan(root, adoption, head, event_source, metrics, *, budget, finals, window):
            self.assertEqual((adoption, head, event_source, budget),
                             (COMMITS[0], COMMITS[3], COMMITS[3], queue.BUDGET_SECONDS))
            self.assertEqual(finals("history"), "read")
            window.update(start=COMMITS[2], revisions=2, adoption=adoption, bound_by=["eg"])
            return {"head": head, "event_source": event_source, "changes": []}
        status, outputs, stdout, reader = self.run_main(plan)
        self.assertEqual(status, 0)
        reader.assert_called_once_with(Path.cwd(), "synthetic-not-a-credential", "history")
        self.assertIn(f"PLAN window start={COMMITS[2]} revisions=2 adoption={COMMITS[0]} bound-by=eg\n", stdout)
        self.assertNotIn("::warning", stdout)

    def test_an_unset_adoption_and_a_wide_window_plan_and_warn(self):
        def plan(root, adoption, head, event_source, metrics, *, budget, finals, window):
            self.assertIsNone(adoption)
            window.update(start=COMMITS[2], revisions=400, adoption="none", bound_by=["eg"])
            return {"head": head, "event_source": event_source, "changes": []}
        status, _outputs, stdout, _reader = self.run_main(plan, unset=("CD_ADOPTION_SHA",))
        self.assertEqual(status, 0)
        self.assertIn("::warning title=Wide release plan::The plan reconciles 400 first-parent merges", stdout)

    def test_a_refused_plan_saves_nothing_planned_and_its_gate_names_the_reason(self):
        def plan(*_args, **_kwargs):
            raise PlanRefused("floor", ["eg"])
        status, outputs, stdout, _reader = self.run_main(plan)
        self.assertEqual((status, outputs), (0, "complete=false\nrefused=floor\ntransient=false\n"))
        self.assertIn("PLAN refused reason=floor lanes=eg\n", stdout)

        def outage(*_args, **_kwargs):
            raise PlanRefused("unreadable", LANES)
        status, outputs, _stdout, _reader = self.run_main(outage)
        self.assertEqual((status, outputs), (0, "complete=false\nrefused=unreadable\ntransient=true\n"))
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
