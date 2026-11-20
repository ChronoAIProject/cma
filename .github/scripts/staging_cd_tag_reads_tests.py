"""Release checks re-read live refs but each immutable tag object at most once."""
import collections
import contextlib
import io
import json
import os
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

import staging_cd_finalize
import staging_cd_git_tests as fixtures
import staging_cd_publish
import staging_cd_release_ownership
import staging_cd_resolve
from staging_cd_allocation_io import AllocationIO
from staging_cd_allocation_snapshot import AllocationSnapshot
from staging_cd_common import Refusal
from staging_cd_github import GitHub
from staging_cd_receipts import annotation, canonical
from staging_cd_receipts_tests import fixture_receipt
from staging_cd_release_calls_tests import serve
from staging_cd_release_tags import REMOTE, LocalTags
from staging_cd_reservations import (ReservationSuperseded, Reservations, claim_ref, next_reservation,
                                     pending_name, validate_reservation)


class Stop(Exception):
    """Ends an entry point once its GitHub client has been constructed."""


class TagReadTests(unittest.TestCase):
    def test_repeated_checks_read_each_tag_object_once_and_every_inventory_again(self):
        receipt = fixture_receipt()
        oid = "a" * 40
        responses = {
            "git/matching-refs/tags/cma-web/v": [{"ref": "refs/tags/" + receipt["tag"],
                                                 "object": {"type": "tag", "sha": oid}}],
            "git/tags/" + oid: {"sha": oid, "tag": receipt["tag"], "message": annotation(receipt),
                                "object": {"type": "commit", "sha": receipt["source"]}},
        }
        api = GitHub("fixture")
        def runner(args, **_kwargs):
            return Mock(returncode=0, stdout=json.dumps(responses[args[-1].split("/cma/", 1)[1]]))
        with patch.object(api, "runner", side_effect=runner) as remote, patch.object(api, "history"):
            for _ in range(3):
                record, = api.tags("web")
                self.assertEqual(record["receipt"], receipt)
            # A caller mutating a returned object cannot change the remembered one.
            api.request("git/tags/" + oid)["message"] = "mutated"
            self.assertEqual(api.request("git/tags/" + oid)["message"], annotation(receipt))
        self.assertEqual([c.args[0][-1].split("/cma/", 1)[1] for c in remote.call_args_list],
                         ["git/matching-refs/tags/cma-web/v", "git/tags/" + oid,
                          "git/matching-refs/tags/cma-web/v", "git/matching-refs/tags/cma-web/v"])

    def test_every_release_entry_point_reads_tag_objects_from_its_checkout(self):
        # AM is the lane whose finalization spent minutes on per-object API reads.
        reservation = next_reservation("am", "a" * 40, "b" * 40, 1, "patch", 7, 1, [], [])
        with tempfile.TemporaryDirectory() as directory:
            env = {"GH_TOKEN": "fixture", "GITHUB_EVENT_NAME": "push", "CD_COMPONENT": "am",
                   "GITHUB_RUN_ID": "7", "GITHUB_RUN_ATTEMPT": "1", "CD_RESOLVE_MODE": "verify",
                   "CD_RECEIPTS_PATH": str(Path(directory) / "receipts.json"),
                   "CD_TASKS": json.dumps([{"component": "am", "mode": "publish", "reservation": reservation}])}
            for module in (staging_cd_resolve, staging_cd_finalize, staging_cd_publish):
                with self.subTest(entry=module.__name__):
                    github = Mock(side_effect=Stop())
                    with patch.dict(os.environ, env, clear=True), patch.object(module, "GitHub", github), \
                            patch.object(module, "Registry"), \
                            patch.object(module, "event_identity", create=True), \
                            contextlib.redirect_stdout(io.StringIO()):
                        self.assertNotEqual(module.main(), 0)
                    self.assertEqual(getattr(github.call_args.kwargs.get("local_tags"), "root", None), Path.cwd())
            store = Mock()
            with patch.object(staging_cd_release_ownership, "event_identity"):
                self.assertTrue(staging_cd_release_ownership.owned(env, Path(directory), reservations=store))
            self.assertEqual(getattr(store.call_args.args[0].local_tags, "root", None), Path(directory))



class BurnedReservationPruningTests(unittest.TestCase):
    """Real refs in a disposable bare remote; one lane finalizes while older reservations stay burned."""

    def setUp(self):
        self.fixture = fixtures.GitPublicationTests()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        f = self.fixture
        self.sources = [f.source]
        for index in range(3):
            self.sources.append(f.git("commit-tree", f.tree, "-p", self.sources[-1], data=f"Merge {index}\n"))
        f.git("push", str(f.remote), self.sources[-1] + ":refs/heads/develop")
        self.departed = f.git("commit-tree", f.tree, data="Source that left develop\n")
        self.pushes = []
        runner = f.publisher.runner
        def observe(args, **kwargs):
            if args[1] == "push":
                self.pushes.append(args)
            return runner(args, **kwargs)
        f.publisher.runner = observe

    def reserve(self, version, previous, source, run_id, *, level="patch", claim=True):
        value = validate_reservation({"schema": "cma-version-reservation-v1", "component": "web", "version": version,
                                      "previous": previous, "source": source, "tree": self.fixture.tree, "pr": 42,
                                      "bump": level, "run_id": run_id, "run_attempt": 1})
        self.assertTrue(self.fixture.publisher.insert(value))
        if claim:
            self.claim(value, self.oid("refs/tags/" + pending_name(value)))
        return value

    def oid(self, ref):
        return self.fixture.git("rev-parse", ref, remote=True)

    def claim(self, value, oid):
        self.fixture.git("update-ref", claim_ref(value), oid, remote=True)

    def refs(self, prefix):
        return set(self.fixture.git("for-each-ref", "--format=%(refname)", prefix, remote=True).split())

    def refs_oids(self, prefix):
        return self.fixture.git("for-each-ref", "--format=%(objectname)", prefix, remote=True).split()

    def lane(self):
        f, (s0, s1, s2, s3) = self.fixture, self.sources
        burned = [self.reserve("0.10.1", "0.10.0", s0, 1), self.reserve("0.10.2", "0.10.1", s1, 2, claim=False)]
        # A finalized release whose cleanup never ran leaves its own reservation behind.
        settled = self.reserve("0.10.3", "0.10.2", s1, 3)
        with patch.object(f.publisher, "remove_after_publication"):
            self.assertTrue(f.publisher.finalize(f.receipt(settled), lambda *_: True))
        burned.append(self.reserve("0.10.4", "0.10.3", self.departed, 4))
        own = self.reserve("0.10.5", "0.10.4", s2, 5)
        current = self.reserve("0.11.0", "0.10.5", s2, 5, level="minor")
        queued = self.reserve("0.11.1", "0.11.0", s3, 6)
        return burned, settled, own, current, queued

    def checkout_client(self):
        """The production client: live refs from the remote's API, objects from the checkout."""
        f, calls, objects = self.fixture, collections.Counter(), []
        def local(args, **kwargs):
            if args[-1] == "--batch":
                objects.extend(kwargs["input"].decode().split())
            return f.runner(args, **kwargs)
        def api(args, **kwargs):
            if args[0] != "gh":
                return f.runner(args, **kwargs)
            return serve(lambda *command, data=None: f.git(*command, remote=True, data=data), args, calls)
        client = GitHub("fixture", api, local_tags=LocalTags(f.work, remote=REMOTE, runner=local))
        client.history = f.api.history
        return client, calls, objects

    def test_finalized_lane_prunes_only_reservations_its_finals_fence_and_their_unbacked_claims(self):
        f = self.fixture
        burned, settled, own, current, queued = self.lane()
        receipt = f.receipt(current)
        before = set(self.refs_oids("refs/tags/cma-web/"))
        api, calls, objects = self.checkout_client()
        publisher = Reservations(api, "fixture", f.work, f.publisher.runner)
        self.assertTrue(publisher.finalize(receipt, lambda *_: True))
        claims = self.refs("refs/tags/cma-allocation/")
        self.pushes.clear()
        self.assertEqual(publisher.prune_burned(receipt), 4)
        # Every annotation came from the checkout, each read once, however often its ref was listed.
        self.assertNotIn("api git/tags/<oid>", calls)
        self.assertGreater(calls["api git/matching-refs/tags/cma-web/pending/v"], 1)
        self.assertEqual(len(objects), len(set(objects)))
        self.assertEqual(set(objects), before | set(self.refs_oids("refs/tags/cma-web/")))
        # Burned versions above every final and the current run's reservations remain.
        self.assertEqual(f.publisher.list("web"), [own, queued])
        removed = {claim_ref(value) for value in burned if value["version"] != "0.10.2"}
        self.assertEqual(self.refs("refs/tags/cma-allocation/"), claims - removed)
        self.assertIn(claim_ref(settled), claims - removed)
        push, = self.pushes
        self.assertEqual(push[:4], ["git", "push", "--atomic", "--porcelain"])
        leases = [arg for arg in push if arg.startswith("--force-with-lease=")]
        deleted = [arg for arg in push if arg.startswith(":refs/tags/")]
        self.assertEqual(len(leases), len(deleted), 6)
        self.assertFalse(any(arg in ("--force", "-f") or arg.startswith("+") for arg in push))
        self.assertEqual(f.publisher.prune_burned(receipt), 0)
        self.assertEqual(len(self.pushes), 1)
        # A queued run still holding a pruned reservation yields to the same final.
        for value in burned[:1] + [settled]:
            with self.assertRaises(ReservationSuperseded):
                f.publisher.require_current(value, build_started=False)
            with self.assertRaisesRegex(Refusal, "missing"):
                f.publisher.require_current(value, build_started=True)
        # Allocation still finds every remaining claim backed and plans above the final.
        store = AllocationSnapshot(f.api, "fixture", f.work, AllocationIO(f.runner))
        with patch.object(f.api, "request", side_effect=self.annotated(f.request)):
            store.load()
        self.assertEqual(store.list("web"), [own, queued])
        self.assertEqual(next_reservation("web", self.sources[-1], f.tree, 43, "patch", 7, 1,
                                          f.api.tags("web"), store.list("web"))["version"], "0.11.2")

    def test_a_claim_unlisted_by_a_lagging_api_still_leaves_with_its_reservation(self):
        f = self.fixture
        burned, _, own, current, queued = self.lane()
        receipt = f.receipt(current)
        self.assertTrue(f.publisher.finalize(receipt, lambda *_: True))
        # GitHub's ref listings can trail Git. Allocation pushed each reservation
        # with its claim atomically, and one advertisement shows both or neither.
        request = f.request
        def lagging(path, **_kwargs):
            return [] if path.startswith("git/matching-refs/tags/cma-allocation/") else request(path)
        commands, runner = [], f.publisher.runner
        def observe(args, **kwargs):
            commands.append(args[1])
            return runner(args, **kwargs)
        f.publisher.runner = observe
        with patch.object(f.api, "request", side_effect=lagging):
            self.assertEqual(f.publisher.prune_burned(receipt), 4)
        self.assertFalse({claim_ref(value) for value in burned} & self.refs("refs/tags/cma-allocation/"))
        store = AllocationSnapshot(f.api, "fixture", f.work, AllocationIO(f.runner))
        with patch.object(f.api, "request", side_effect=self.annotated(f.request)):
            store.load()
        self.assertEqual(store.list("web"), [own, queued])
        self.assertEqual(commands, ["ls-remote", "push", "ls-remote"])

    def test_every_pruning_git_call_is_bounded_by_the_finalization_budget(self):
        f = self.fixture
        _, _, own, current, queued = self.lane()
        receipt = f.receipt(current)
        self.assertTrue(f.publisher.finalize(receipt, lambda *_: True))
        before = f.publisher.list("web")
        timeouts, runner = [], f.publisher.runner
        def observe(args, **kwargs):
            timeouts.append((args[1], kwargs["timeout"]))
            return runner(args, **kwargs)
        f.publisher.runner = observe
        # The push would start with three seconds left, so it never starts.
        ticks = iter([0, 97])
        with self.assertRaisesRegex(Refusal, "Pruning budget expired"):
            f.publisher.prune_burned(receipt, deadline=100, clock=lambda: next(ticks))
        self.assertEqual([name for name, _ in timeouts], ["ls-remote"])
        self.assertEqual(f.publisher.list("web"), before)
        timeouts.clear()
        self.assertEqual(f.publisher.prune_burned(receipt, deadline=time.monotonic() + 40), 4)
        self.assertEqual([name for name, _ in timeouts], ["ls-remote", "push", "ls-remote"])
        self.assertTrue(all(0 < seconds <= 40 for _, seconds in timeouts))
        self.assertEqual(f.publisher.list("web"), [own, queued])

    @staticmethod
    def annotated(request):
        def read(path, **_kwargs):
            result = request(path)
            if path.startswith("git/tags/"):
                result["sha"] = path.removeprefix("git/tags/")
            return result
        return read

    def test_limit_conflicting_claims_newer_sources_and_moved_refs_keep_tags(self):
        f, (s0, s1, s2, s3) = self.fixture, self.sources
        first = self.reserve("0.10.1", "0.10.0", s0, 1)
        foreign = self.reserve("0.10.2", "0.10.1", s1, 2, claim=False)
        self.claim(foreign, self.oid("refs/tags/" + pending_name(first)))
        moved = self.reserve("0.10.3", "0.10.2", s1, 3)
        receipt = f.receipt(self.reserve("0.10.4", "0.10.3", s2, 4, claim=False))
        self.assertTrue(f.publisher.finalize(receipt, lambda *_: True))
        # Only an impossible allocation could put a lower version on a newer source.
        newer = self.reserve("0.10.0", "0.9.9", s3, 5, level="minor")
        with patch("staging_cd_reservations.PRUNE_LIMIT", 1):
            self.assertEqual(f.publisher.prune_burned(receipt), 1)
        self.assertCountEqual(f.publisher.list("web"), [foreign, moved, newer])
        # A ref that names another object after the inventory read fails its lease,
        # and the atomic push then keeps the claim it would have taken too.
        runner, name = f.publisher.runner, pending_name(moved)
        def replace(args, **kwargs):
            if args[1] == "push":
                text = (f"object {s1}\ntype commit\ntag {name}\ntagger Other <other@example.invalid> 1 +0000\n\n"
                        + canonical(moved) + "\n")
                f.git("update-ref", "refs/tags/" + name, f.git("mktag", data=text, remote=True), remote=True)
            return runner(args, **kwargs)
        f.publisher.runner = replace
        with self.assertRaisesRegex(Refusal, "Burned reservation pruning failed"):
            f.publisher.prune_burned(receipt)
        self.assertIn(claim_ref(moved), self.refs("refs/tags/cma-allocation/"))
        self.assertCountEqual(f.publisher.list("web"), [foreign, moved, newer])

    def test_finalization_reports_pruning_and_never_fails_a_published_release_over_it(self):
        receipt = fixture_receipt()
        retained = "result=retained removed=0"
        for outcome, line in ((3, "result=complete removed=3"), (Refusal("GitHub API request failed"), retained),
                              (RuntimeError("private detail"), retained), (AttributeError("private"), retained)):
            with self.subTest(outcome=outcome), tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "receipts.json"
                path.write_text("[]")
                env = {"GITHUB_EVENT_NAME": "push", "GITHUB_RUN_ID": "1", "GITHUB_RUN_ATTEMPT": "1",
                       "CD_RECEIPTS_PATH": str(path), "CD_TASKS": "[]", "GH_TOKEN": "fixture", "CD_COMPONENT": "web"}
                stdout = io.StringIO()
                with patch.dict(os.environ, env, clear=True), patch.object(staging_cd_finalize, "event_identity"), \
                        patch.object(staging_cd_finalize, "GitHub"), patch.object(staging_cd_finalize, "Registry"), \
                        patch.object(staging_cd_finalize, "Reservations") as store, \
                        patch.object(staging_cd_finalize, "finalize", return_value=receipt), \
                        patch.object(staging_cd_finalize, "output"), contextlib.redirect_stdout(stdout):
                    store.return_value.prune_burned.side_effect = [outcome]
                    self.assertEqual(staging_cd_finalize.main(), 0)
                store.return_value.prune_burned.assert_called_once()
                self.assertEqual(store.return_value.prune_burned.call_args.args, (receipt,))
                self.assertIn("PRUNE finalize component=web " + line + "\n", stdout.getvalue())
                self.assertEqual("::warning" in stdout.getvalue(), isinstance(outcome, Exception))
                self.assertNotIn("private", stdout.getvalue())
        # Too little of the finalization budget left: no pruning starts at all.
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "receipts.json"
            path.write_text("[]")
            env = {"GITHUB_EVENT_NAME": "push", "GITHUB_RUN_ID": "1", "GITHUB_RUN_ATTEMPT": "1",
                   "CD_RECEIPTS_PATH": str(path), "CD_TASKS": "[]", "GH_TOKEN": "fixture", "CD_COMPONENT": "web"}
            stdout = io.StringIO()
            with patch.dict(os.environ, env, clear=True), patch.object(staging_cd_finalize, "event_identity"), \
                    patch.object(staging_cd_finalize, "FINALIZE_READ_SECONDS", staging_cd_finalize.PRUNE_MIN_SECONDS - 1), \
                    patch.object(staging_cd_finalize, "GitHub"), patch.object(staging_cd_finalize, "Registry"), \
                    patch.object(staging_cd_finalize, "Reservations") as store, \
                    patch.object(staging_cd_finalize, "finalize", return_value=receipt), \
                    patch.object(staging_cd_finalize, "output"), contextlib.redirect_stdout(stdout):
                self.assertEqual(staging_cd_finalize.main(), 0)
            store.return_value.prune_burned.assert_not_called()
            self.assertIn("PRUNE finalize component=web result=skipped removed=0\n", stdout.getvalue())
            self.assertIn("::warning title=Burned reservations retained::", stdout.getvalue())
        for interrupt in (KeyboardInterrupt, SystemExit):
            with self.subTest(interrupt=interrupt), self.assertRaises(interrupt):
                staging_cd_finalize.prune(Mock(prune_burned=Mock(side_effect=interrupt)), receipt)


if __name__ == "__main__":
    unittest.main()
