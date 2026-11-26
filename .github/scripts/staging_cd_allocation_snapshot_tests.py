"""Single-snapshot allocation preserves authority with exact-ref settlement."""
import json
from pathlib import Path
import subprocess
from types import SimpleNamespace
import unittest
from unittest.mock import Mock

from staging_cd_allocate import TRANSIENT_CODES, allocate_queue, refusal_code
from staging_cd_allocation_fixture import AllocationServer
from staging_cd_allocation_io import (ALLOCATION_SECONDS, PUSH_RESERVE_SECONDS, PUSH_SECONDS, SETTLE_WAITS,
                                      VERIFY_RESERVE_SECONDS, AllocationIO)
from staging_cd_allocation_snapshot import AllocationSnapshot, SnapshotAPI
from staging_cd_common import Refusal
from staging_cd_github import GitHubNotFound
from staging_cd_receipts_tests import fixture_receipt
from staging_cd_receipts import canonical
from staging_cd_reservations import next_reservation, pending_name
from staging_cd_versions import DEPLOYABLES

REMOTE = "https://github.com/ChronoAIProject/cma.git"


def porcelain(summary):
    """git push --porcelain output in which every ref received one summary."""
    return f"To {REMOTE}\n!\t{'f' * 40}:refs/tags/cma-agent-manager/pending/v0.10.4\t{summary}\nDone\n"


class AllocationSnapshotTests(unittest.TestCase):
    def fixture(self, count=3, **kwargs):
        server = AllocationServer(count, **kwargs)
        store = AllocationSnapshot(server, "fixture", Path.cwd(), AllocationIO(server.command))
        return server, store

    def allocate(self, server, store):
        store.load()
        matrix = allocate_queue(server.queue(), SnapshotAPI(server, store), store, 999, 1)
        store.commit()
        return matrix

    def test_one_snapshot_one_push_and_no_older_owner_read(self):
        for component in sorted(DEPLOYABLES):
            with self.subTest(component=component):
                server, store = self.fixture(25, latency=0.001, components=(component,))
                matrix = self.allocate(server, store)
                tasks = json.loads(matrix["include"][0]["tasks"])
                self.assertEqual(len(tasks), 1)
                self.assertEqual(tasks[0]["reservation"]["version"], "0.10.27" if component == "runtime" else "0.10.26")
                self.assertEqual([args[1] for args in server.commands].count("ls-remote"), 1)
                pushes = [args for args in server.commands if args[1] == "push"]
                self.assertEqual(len(pushes), 1)
                # The reservation and its claim; a started predecessor keeps its outcome ref.
                self.assertEqual(len([arg for arg in pushes[0] if ":refs/tags/" in arg]), 2)
                self.assertFalse(any("cma-allocation-outcome/" in arg for arg in pushes[0]))
                self.assertEqual([path for path in server.calls if path.startswith("actions/")],
                                 [])
                self.assertFalse(any("matching-refs" in path for path in server.calls))
                self.assertGreater(server.peak, 1)
                self.assertLessEqual(server.peak, 8)
                self.assertTrue(all(server.refs[name] == oid for name, oid in server.original.items()))
                with self.assertRaises(Refusal):
                    store.load()

    def test_fifteen_queued_merges_reserve_only_five_distinct_latest_sources(self):
        server, store = self.fixture(14, reserved=False, components=tuple(DEPLOYABLES))
        queue = server.queue()
        # All 15 change the first component. Each peer stops changing one merge earlier.
        for index, change in enumerate(queue["changes"]):
            change["deployables"] = sorted(name for offset, name in enumerate(DEPLOYABLES.values())
                                           if index <= 14 - offset)
        queue["changes"].reverse()
        store.load()
        result = allocate_queue(queue, SnapshotAPI(server, store), store, 99, 1)
        store.commit()
        self.assertEqual(len(result["include"]), 5)
        expected = {key: server.sources[14 - i] for i, key in enumerate(DEPLOYABLES)}
        self.assertEqual({item["component"]: item["source"] for item in store.planned}, expected)
        self.assertEqual({item["component"]: item["version"] for item in store.list()},
                         {key: "0.10.2" if key == "runtime" else "0.10.1" for key in expected})
        self.assertEqual(len([args for args in server.commands if args[1] == "push"]), 1)
        self.assertEqual(len([path for path in server.calls if path.startswith("git/ref/tags/")]), 5)
        self.assertEqual({p.split("/")[1] for p in server.calls if p.startswith("commits/")}, set(expected.values()))

    def test_same_source_active_owner_is_still_respected(self):
        server, store = self.fixture()
        queue = server.queue()
        queue["changes"].pop()
        request = server.request
        def active(path, **kwargs):
            result = request(path, **kwargs)
            if path.startswith("actions/"):
                self.assertEqual(path, "actions/runs/12/attempts/1")
                return dict(result, status="in_progress")
            return result
        server.request = active
        store.load()
        self.assertEqual(allocate_queue(queue, SnapshotAPI(server, store), store, 999, 1), {"include": []})
        store.commit()
        self.assertFalse(any(args[1] == "push" for args in server.commands))

    def test_inventory_refusal_has_no_write_or_partial_success(self):
        for output in ("invalid", "a" * 40 + "\trefs/tags/cma-agent-manager/pending/vinvalid\n",
                       ("a" * 40 + "\trefs/tags/fixture\n") * 2):
            server, store = self.fixture()
            store.io.runner = Mock(return_value=subprocess.CompletedProcess([], 0, output, ""))
            with self.assertRaises(Refusal):
                store.load()
            self.assertFalse(store.loaded)
            self.assertEqual(store.planned, [])

    def test_annotation_binding_and_partial_reads_fail_closed(self):
        for field, value in (("sha", "0" * 40), ("tag", "wrong"), ("object", {"type": "commit", "sha": "e" * 40}),
                             ("message", "{}")):
            server, store = self.fixture()
            server.objects[next(iter(server.objects))][field] = value
            with self.assertRaises(Refusal):
                store.load()
            self.assertFalse(store.loaded)
            self.assertFalse(any(args[1] == "push" for args in server.commands))

    def test_final_receipt_is_bound_to_snapshot_oid_and_history(self):
        server, store = self.fixture()
        receipt = fixture_receipt(source=server.sources[-1])
        oid = "f" * 40
        server.refs[receipt["tag"]] = oid
        server.objects[oid] = {"sha": oid, "tag": receipt["tag"], "message": canonical(receipt),
                               "object": {"type": "commit", "sha": receipt["source"]}}
        store.load()
        self.assertEqual(store.finals[0]["receipt"], receipt)
        server.objects[oid]["object"]["sha"] = server.sources[0]
        replacement = AllocationSnapshot(server, "fixture", Path.cwd(), AllocationIO(server.command))
        with self.assertRaisesRegex(Refusal, "receipt"):
            replacement.load()

    def test_collision_and_mismatched_readback_never_retry_push(self):
        for mode in ("collision", "corrupt_readback"):
            server, store = self.fixture()
            setattr(server, mode, True)
            with self.assertRaises(Refusal):
                self.allocate(server, store)
            self.assertEqual(len([args for args in server.commands if args[1] == "push"]), 1)
            self.assertTrue(all(server.refs[name] == oid for name, oid in server.original.items()))

    def test_a_push_github_declined_with_nothing_landed_is_rejected_for_good(self):
        denied = (f"remote: Permission to ChronoAIProject/cma.git denied to github-actions[bot].\n"
                  f"fatal: unable to access '{REMOTE}/': The requested URL returned error: 403\n")
        outage = f"fatal: unable to access '{REMOTE}/': The requested URL returned error: 502\n"
        reset = "error: RPC failed; curl 56 Recv failure: Connection reset by peer\n"
        forbidden = f"fatal: unable to access '{REMOTE}/': The requested URL returned error: 403\n"
        for name, rejection, message in (
                ("ruleset", (1, porcelain("[remote rejected] (push declined due to repository rule violations)"),
                             "remote: error: GH013: Repository rule violations found\n"), "push was rejected"),
                ("hook", (1, porcelain("[remote rejected] (pre-receive hook declined)"), ""), "push was rejected"),
                ("permission", (128, "", denied), "push was rejected"),
                ("write access", (128, "", "remote: Write access to repository not granted.\n" + forbidden),
                 "push was rejected"),
                # A 403 is durable only alone and with the permission it lacks.
                ("secondary rate limit", (128, "", "remote: You have exceeded a secondary rate limit.\n" + forbidden),
                 "readback differs"),
                ("rate limit and permission", (128, "", "remote: API rate limit exceeded.\n" + denied),
                 "readback differs"),
                ("bare 403", (128, "", forbidden), "readback differs"),
                ("permission without its 403", (128, "", denied.splitlines()[0] + "\n"), "readback differs"),
                ("403 then 502", (128, "", denied + outage), "readback differs"),
                ("authentication", (128, "", "remote: Invalid username or token.\n"
                                    f"fatal: Authentication failed for '{REMOTE}/'\n"), "readback differs"),
                # Without GitHub's own refusal, a failed push proves nothing durable.
                ("outage", (128, "", outage), "readback differs"),
                ("reset", (128, "", reset), "readback differs"),
                ("unreported", (1, porcelain("[remote failure] (remote failed to report status)"), ""),
                 "readback differs"),
                ("server error", (1, porcelain("[remote rejected] (Internal Server Error)"), ""), "readback differs"),
                ("silent", (1, "", ""), "readback differs")):
            with self.subTest(case=name):
                server, store = self.fixture()
                server.rejection = rejection
                with self.assertRaisesRegex(Refusal, message) as raised:
                    self.allocate(server, store)
                self.assertEqual(refusal_code(raised.exception), "RESERVATION_PUSH_REJECTED"
                                 if message == "push was rejected" else "RESERVATION_VERIFY_FAILED")
                self.assertEqual(len([args for args in server.commands if args[1] == "push"]), 1)
                self.assertEqual(server.refs, server.original)

    def test_a_declined_push_with_anything_listed_is_not_a_clean_rejection(self):
        # GitHub declined, but a planned ref already holds another object:
        # that is not "nothing landed", so it stays a readback refusal.
        server, store = self.fixture()
        server.rejection = (1, porcelain("[remote rejected] (pre-receive hook declined)"), "")
        store.load()
        allocate_queue(server.queue(), SnapshotAPI(server, store), store, 999, 1)
        server.refs[pending_name(store.planned[0])] = "0" * 40
        with self.assertRaisesRegex(Refusal, "readback differs") as raised:
            store.commit()
        self.assertEqual(refusal_code(raised.exception), "RESERVATION_VERIFY_FAILED")

    def test_an_oversized_inventory_is_refused_apart_from_an_incomplete_read(self):
        for output, returncode, message, code in (
                ("x" * (16 * 1024 * 1024 + 1), 0, "exceeds 16 MiB", "INVENTORY_TOO_LARGE"),
                ("", 128, "inventory is incomplete", "INVENTORY_INCOMPLETE")):
            server, store = self.fixture()
            store.io.runner = Mock(return_value=subprocess.CompletedProcess([], returncode, output,
                                                                            "fatal: protocol error\n"))
            with self.subTest(code=code), self.assertRaisesRegex(Refusal, message) as raised:
                store.load()
            self.assertEqual(refusal_code(raised.exception), code)
            self.assertFalse(store.loaded)

    def test_a_duplicate_or_retired_planned_reservation_is_a_planner_defect(self):
        server, store = self.fixture()
        store.load()
        fresh = next_reservation("am", server.sources[-1], "e" * 40, 4, "patch", 999, 1, [], store.list())
        for value, message in ((dict(store.list()[0]), "planned a duplicate reservation"),
                               (dict(fresh, component="image-manager"), "Unknown reservation component"),
                               (fresh, None),
                               (dict(fresh, run_id=1000), "planned a duplicate reservation")):
            with self.subTest(value=value, message=message):
                if message is None:
                    self.assertTrue(store.insert(value))
                    continue
                with self.assertRaisesRegex(Refusal, message) as raised:
                    store.insert(value)
                self.assertEqual(refusal_code(raised.exception), "RESERVATION_PLAN_INVALID")
        self.assertEqual(store.planned, [fresh])

    def test_ambiguous_response_requires_exact_ref_readback_after_each_push(self):
        server, store = self.fixture(3, reserved=False)
        server.ambiguous = True
        self.allocate(server, store)
        self.assertEqual(len(server.refs), 2)
        self.assertEqual(len([args for args in server.commands if args[1] == "push"]), 1)
        self.assertEqual(store.list(), store.planned)

    def test_expired_push_window_after_building_five_objects_never_writes(self):
        server, store = self.fixture(14, reserved=False, components=tuple(DEPLOYABLES))
        now = [0]
        created = []
        def runner(args, **kwargs):
            if args[1] == "mktag":
                created.append(args)
                if len(created) == 5:
                    now[0] = ALLOCATION_SECONDS - 1
            return server.command(args, **kwargs)
        store.io = AllocationIO(runner, clock=lambda: now[0])
        with self.assertRaisesRegex(Refusal, "deadline"):
            self.allocate(server, store)
        self.assertEqual(server.refs, {})
        self.assertFalse(any(args[1] == "push" for args in server.commands))

    def uncertain_push(self, seen, *, left=ALLOCATION_SECONDS, latency=0.0):
        """Five reservations whose atomic push times out after its whole client bound.

        The server holds the pushed refs back. For the n-th read attempt after
        the push, `seen(n)` returns "absent", "commit" (the whole push becomes
        visible), "partial" (one claim stays absent), "foreign" (one claim holds
        another object), or an exception or process result that replaces the
        read. The push starts with `left` seconds of allocation budget, and each
        read takes `latency` seconds.
        """
        server, store = self.fixture(14, reserved=False, components=tuple(DEPLOYABLES))
        case = SimpleNamespace(server=server, store=store, now=[0.0], held={}, reads=0, waits=[], marks=[])

        def runner(args, **kwargs):
            if args[1] == "push":
                server.command(args, **kwargs)
                case.held.update((name, server.refs.pop(name)) for name in list(server.refs))
                case.now[0] += kwargs["timeout"]
                raise subprocess.TimeoutExpired("git", kwargs["timeout"])
            if args[1] == "ls-remote" and case.held:
                case.reads += 1
                case.now[0] += latency
                effect = seen(case.reads)
                if isinstance(effect, BaseException):
                    raise effect
                if isinstance(effect, subprocess.CompletedProcess):
                    return effect
                claim = next(name for name in case.held if name.startswith("cma-allocation/"))
                visible = {"absent": {}, "commit": case.held, "foreign": {**case.held, claim: "0" * 40},
                           "partial": {name: oid for name, oid in case.held.items() if name != claim}}[effect]
                server.refs = dict(visible)
            return server.command(args, **kwargs)

        def sleep(seconds):
            case.waits.append(seconds)
            case.now[0] += seconds
        store.io = AllocationIO(runner, clock=lambda: case.now[0], sleep=sleep, rand=lambda: 1.0, seconds=left)
        store.progress = lambda _component, phase: case.marks.append(phase)
        return case

    def assert_settlement(self, case, *, settled, waits, polls, reads=None):
        server, store = case.server, case.store
        if settled:
            self.allocate(server, store)
            self.assertEqual(len(store.list()), 5)
            self.assertEqual(server.refs, case.held)
        else:
            with self.assertRaisesRegex(Refusal, "settlement is uncertain"):
                self.allocate(server, store)
            self.assertEqual(store.list(), [])
        self.assertEqual(case.waits, waits)
        self.assertEqual((case.marks.count("settle-poll"), case.reads), (polls, polls if reads is None else reads))
        self.assertEqual([mark for mark in case.marks if mark in ("settled", "settle-refused")],
                         ["settled" if settled else "settle-refused"])
        # Only a settled batch reads its owners back; the push is never replayed.
        self.assertEqual(len([p for p in server.calls if p.startswith("git/ref/tags/")]), 5 if settled else 0)
        self.assertEqual(len([args for args in server.commands if args[1] == "push"]), 1)
        self.assertLess(case.now[0], store.io.deadline)

    def test_timed_out_atomic_push_settles_only_the_exact_batch_without_replay(self):
        # "late" is the hosted failure of run 44948149418: the client timed out,
        # then the server committed the whole atomic push a few seconds later.
        # A push that starts early has 120 - 30 - 5 s to settle: waits of 1, 2,
        # 4 and 8 s, then 8 s each, and a last wait that ends at the reserve.
        until_reserve = [1, 2, 4] + [8] * 9 + [6]
        self.assertEqual(sum(until_reserve), ALLOCATION_SECONDS - PUSH_SECONDS - VERIFY_RESERVE_SECONDS)
        for name, seen, settled, waits in (
                ("committed", lambda _poll: "commit", True, []),
                ("late", lambda poll: "commit" if poll >= 4 else "absent", True, [1, 2, 4]),
                ("79 s late", lambda poll: "commit" if poll >= 13 else "absent", True, until_reserve[:12]),
                ("partial", lambda _poll: "partial", False, until_reserve),
                ("never", lambda _poll: "absent", False, until_reserve),
                ("foreign", lambda _poll: "foreign", False, []),
                ("foreign after waiting", lambda poll: "foreign" if poll >= 3 else "absent", False, [1, 2])):
            with self.subTest(case=name):
                self.assert_settlement(self.uncertain_push(seen), settled=settled, waits=waits,
                                       polls=len(waits) + 1)

    def test_settlement_with_slow_reads_stops_at_the_verification_reserve(self):
        # The push starts at the latest allowed moment and uses its whole bound.
        # Each 1 s read eats into the waits, so the fourth wait is cut to what
        # leaves the reserve, and no fifth wait starts: refused before the deadline.
        case = self.uncertain_push(lambda _poll: "absent", left=PUSH_RESERVE_SECONDS, latency=1.0)
        self.assert_settlement(case, settled=False, waits=[1, 2, 4, 4], polls=5)
        self.assertEqual(case.store.io.deadline - case.now[0], VERIFY_RESERVE_SECONDS - 1)

    def test_push_at_the_latest_allowed_moment_still_gets_every_wait(self):
        for settled in (True, False):
            with self.subTest(settled=settled):
                case = self.uncertain_push(lambda poll: "commit" if settled and poll == 5 else "absent",
                                           left=PUSH_RESERVE_SECONDS)
                self.assert_settlement(case, settled=settled, waits=list(SETTLE_WAITS), polls=5)
                # The last read starts with exactly the verification reserve left.
                self.assertEqual(case.store.io.deadline - case.now[0], VERIFY_RESERVE_SECONDS)

    def test_transient_settlement_reads_keep_polling_until_the_push_is_seen(self):
        outage = "fatal: unable to access 'https://github.com/ChronoAIProject/cma.git/': The requested URL returned error: 502\n"
        hangup = "fatal: the remote end hung up unexpectedly\n"
        for name, failure, left, waits, reads in (
                # Late in the budget each poll is a single read attempt.
                ("http 502", subprocess.CompletedProcess([], 128, "", outage), PUSH_RESERVE_SECONDS, [1, 2], 3),
                ("hung up", subprocess.CompletedProcess([], 128, "", hangup), PUSH_RESERVE_SECONDS, [1, 2], 3),
                # Earlier, the middle poll repeats its read twice before it fails.
                ("three timeouts", subprocess.TimeoutExpired("git", 5), ALLOCATION_SECONDS, [1, 0.5, 1.0, 2], 5)):
            with self.subTest(failure=name):
                case = self.uncertain_push(
                    lambda read: failure if 1 < read < reads else ("commit" if read == reads else "absent"), left=left)
                self.assert_settlement(case, settled=True, waits=waits, polls=3, reads=reads)
                self.assertEqual(case.marks.count("settle-transient"), 1)

    def test_permanent_settlement_read_failure_refuses_at_once(self):
        missing = "fatal: repository 'https://github.com/ChronoAIProject/cma.git/' not found\n"
        failure = subprocess.CompletedProcess([], 128, "", missing)
        case = self.uncertain_push(lambda poll: failure if poll == 2 else "absent")
        with self.assertRaisesRegex(Refusal, "inventory is incomplete"):
            self.allocate(case.server, case.store)
        self.assertEqual((case.waits, case.reads), ([1], 2))
        self.assertNotIn("settle-transient", case.marks)
        self.assertEqual(len([args for args in case.server.commands if args[1] == "push"]), 1)

    def test_transient_inventory_read_is_repeated_instead_of_refused(self):
        outage = "fatal: unable to access 'https://github.com/ChronoAIProject/cma.git/': The requested URL returned error: 502\n"
        server, store = self.fixture()
        failures = [subprocess.CompletedProcess([], 128, "", outage)]
        def runner(args, **kwargs):
            if args[1] == "ls-remote" and failures:
                return failures.pop()
            return server.command(args, **kwargs)
        store.io = AllocationIO(runner, sleep=Mock(), rand=lambda: 1.0)
        self.assertIs(store.load(), store)
        store.io.sleep.assert_called_once_with(0.5)

    def test_missing_readbacks_retry_only_the_exact_ref_then_settle_or_refuse(self):
        for misses in (2, 3):
            server, store = self.fixture(0)
            request, marks = server.request, []
            store.progress = lambda *args: marks.append(args)
            store.io.sleep = Mock()
            reads = []
            def delayed(path, **kwargs):
                if path.startswith("git/ref/tags/"):
                    reads.append(path)
                    if len(reads) <= misses:
                        raise GitHubNotFound("GitHub resource was not found")
                return request(path, **kwargs)
            server.request = delayed
            if misses == 2:
                self.allocate(server, store)
                self.assertEqual(store.list(), store.planned)
                self.assertIn(("am", "reservation-readback"), marks)
            else:
                with self.assertRaisesRegex(Refusal, "unavailable"):
                    self.allocate(server, store)
                self.assertEqual(store.list(), [])
                self.assertEqual(len(server.refs), 2)  # Remote version remains burned.
            self.assertEqual(len(reads), 3)
            self.assertEqual(len(set(reads)), 1)
            self.assertEqual(store.io.sleep.call_count, 2)
            self.assertEqual(marks.count(("am", "readback-missing")), misses)
            self.assertEqual(len([args for args in server.commands if args[1] == "push"]), 1)
            self.assertEqual(len([args for args in server.commands if args[1] == "ls-remote"]), 1)
            with self.assertRaisesRegex(Refusal, "already attempted"):
                store.commit()

    def test_conflicting_or_unreadable_ref_refuses_without_visibility_retries(self):
        for response in (Refusal("private transport"), {},
                         {"ref": "refs/tags/cma-agent-manager/pending/v0.10.1",
                          "object": {"type": "commit", "sha": "a" * 40}}):
            server, store = self.fixture(0)
            request, reads, marks = server.request, [], []
            store.progress = lambda *args: marks.append(args)
            store.io.sleep = Mock()
            def invalid(path, **kwargs):
                if path.startswith("git/ref/tags/"):
                    reads.append(path)
                    if isinstance(response, Exception):
                        raise response
                    return response
                return request(path, **kwargs)
            server.request = invalid
            with self.assertRaises(Refusal):
                self.allocate(server, store)
            self.assertEqual(len(reads), 1)
            store.io.sleep.assert_not_called()
            self.assertEqual(store.list(), [])
            if not isinstance(response, Exception):
                self.assertIn(("am", "readback-conflict"), marks)

    def test_five_components_keep_one_inventory_and_update_only_verified_memory(self):
        server, store = self.fixture(4, components=tuple(DEPLOYABLES))
        matrix = self.allocate(server, store)
        self.assertEqual(len(matrix["include"]), 5)
        self.assertEqual(len(store.list()), 25)
        self.assertEqual(len([args for args in server.commands if args[1] == "push"]), 1)
        self.assertEqual(len([args for args in server.commands if args[1] == "ls-remote"]), 1)
        self.assertEqual(len([path for path in server.calls if path.startswith("git/ref/tags/")]), 5)
        self.assertEqual(len([path for path in server.calls if path.startswith("actions/")]), 0)
        for group in matrix["include"]:
            expected = "0.10.6" if group["component"] == "runtime" else "0.10.5"
            self.assertEqual(json.loads(group["tasks"])[0]["reservation"]["version"], expected)

    def test_unsupported_upgrade_label_prevents_the_entire_batch(self):
        server, store = self.fixture(3, reserved=False)
        request = server.request
        def unsupported(path, **kwargs):
            result = request(path, **kwargs)
            return dict(result, labels=[{"name": "version-upgrade:other"}]) if path == "pulls/4" else result
        server.request = unsupported
        with self.assertRaisesRegex(Refusal, "version-upgrade"):
            self.allocate(server, store)
        self.assertEqual(store.planned, [])
        self.assertEqual(server.refs, {})

    def test_successor_claims_require_a_matching_burned_reservation(self):
        server, store = self.fixture(0)
        self.allocate(server, store)
        claim = next(name for name in server.refs if name.startswith("cma-allocation/"))
        oid = server.refs[claim]
        server.refs[claim] = "e" * 40
        with self.assertRaisesRegex(Refusal, "claim owner differs") as raised:
            AllocationSnapshot(server, "fixture", Path.cwd(), store.io).load()
        # Both refs are create-only and one push writes them with one object,
        # so even a torn listing never shows two objects: lasting damage.
        self.assertEqual(refusal_code(raised.exception), "INVENTORY_CLAIM_CONFLICT")
        server.refs = {claim: oid}
        with self.assertRaisesRegex(Refusal, "claim has no durable version") as raised:
            AllocationSnapshot(server, "fixture", Path.cwd(), store.io).load()
        # A claim alone is also what a listing torn by a concurrent push shows.
        self.assertEqual(refusal_code(raised.exception), "INVENTORY_CLAIM_ORPHANED")
        self.assertIn("INVENTORY_CLAIM_ORPHANED", TRANSIENT_CODES)

    def test_noop_never_pushes_and_snapshot_is_not_usable_before_load(self):
        server, store = self.fixture()
        with self.assertRaises(Refusal):
            store.list()
        with self.assertRaises(Refusal):
            SnapshotAPI(server, store).tags()
        with self.assertRaises(Refusal):
            store.commit()
        store.load()
        store.commit()
        self.assertFalse(any(args[1] == "push" for args in server.commands))
