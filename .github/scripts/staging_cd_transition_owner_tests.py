"""Unrelated pushes must leave the current contract publisher able to finish."""
import hashlib
import json
from pathlib import Path
import unittest
from unittest.mock import patch

import staging_cd_release_ownership as ownership
from staging_cd_allocate import allocate_queue
from staging_cd_component_report import summarize
import staging_cd_component_report_tests as report_fixtures
from staging_cd_allocation_fixture import AllocationServer
from staging_cd_allocation_io import AllocationIO
from staging_cd_allocation_snapshot import AllocationSnapshot, SnapshotAPI
from staging_cd_common import Refusal
from staging_cd_compatibility import GROUP
from staging_cd_history import DevelopHistory
from staging_cd_receipts import annotation
from staging_cd_receipts_tests import fixture_receipt, fixture_runtime_receipt
from staging_cd_reservations import ReservationSuperseded, Reservations, SourceAlreadyFinal, outcome_ref, pending_name
from staging_cd_versions import DEPLOYABLES


class TransitionOwnerTests(unittest.TestCase):
    def setUp(self):
        self.server = AllocationServer(5, reserved=False)
        self.changes = []
        self.completed = set()
        request = self.server.request

        def read(path, **kwargs):
            response = request(path, **kwargs)
            if path.startswith("actions/runs/"):
                response["status"] = "completed" if response["id"] in self.completed else "in_progress"
            return response

        self.server.request = read

    def push(self, components):
        index = len(self.changes)
        source = self.server.sources[index]
        self.server._history = DevelopHistory(source, list(reversed(self.server.sources[:index + 1])))
        self.changes.append({"source": source, "tree": "e" * 40,
                             "deployables": sorted(DEPLOYABLES[key] for key in components)})
        queue = {"head": source, "event_source": source, "changes": self.changes,
                 "contract_transition": self.server.sources[0]}
        snapshot = AllocationSnapshot(self.server, "fixture", Path.cwd(), AllocationIO(self.server.command))
        self.assertTrue(snapshot.load(event_source=source))
        matrix = allocate_queue(queue, SnapshotAPI(self.server, snapshot), snapshot, 801 + index, 1)
        snapshot.commit()
        tasks = {}
        for row in matrix["include"]:
            task, = json.loads(row["tasks"])
            component = row["component"]
            expected_transition = queue["contract_transition"] if component in GROUP else None
            self.assertEqual(row.get("contract_transition"), expected_transition)
            tasks[component] = task
        return tasks

    def finalize(self, reservation):
        component = reservation["component"]
        make_receipt = fixture_runtime_receipt if component.startswith("runtime") else fixture_receipt
        receipt = make_receipt(**{key: value for key, value in reservation.items() if key != "schema"})
        body = annotation(receipt)
        oid = hashlib.sha1(body.encode()).hexdigest()
        outcome = outcome_ref(component, reservation["version"]).removeprefix("refs/tags/")
        self.assertNotIn(outcome, self.server.refs)
        self.server.objects[oid] = {"sha": oid, "tag": receipt["tag"], "message": body,
                                    "object": {"type": "commit", "sha": receipt["source"]}}
        self.server.refs.update({outcome: oid, receipt["tag"]: oid})
        del self.server.refs[pending_name(reservation)]
        return receipt

    def assert_web_pushes_preserve_owner(self, active, finals):
        refs = self.server.refs
        name = pending_name(active)
        oid = refs[name]
        outcome = outcome_ref(active["component"], active["version"]).removeprefix("refs/tags/")
        for _ in range(2):
            tasks = self.push({"web"})
            self.assertEqual(tasks.keys(), {"web"})
            self.assertEqual(tasks["web"]["mode"], "publish")
            self.assertEqual(refs[name], oid)
            self.assertNotIn(outcome, refs)
            self.finalize(tasks["web"]["reservation"])

        finals[active["component"]] = self.finalize(active)
        pushes_before = sum(args[1] == "push" for args in self.server.commands)
        recovered = self.push(set())
        for key in GROUP:
            self.assertEqual(recovered[key], {"component": key, "mode": "resume", "receipt": finals[key]})
        self.assertEqual(sum(args[1] == "push" for args in self.server.commands), pushes_before)

    def test_two_web_pushes_preserve_am_until_the_exact_transition_can_resume(self):
        original = self.push(GROUP)
        finals = {key: self.finalize(original[key]["reservation"]) for key in GROUP - {"am"}}
        self.assert_web_pushes_preserve_owner(original["am"]["reservation"], finals)

    def test_new_am_takes_over_atomically_then_web_preserves_its_older_runtime_source(self):
        original = self.push(GROUP)
        finals = {"runtime-omarchy": self.finalize(original["runtime-omarchy"]["reservation"])}
        successor = self.push({"am"})
        self.assertEqual(successor.keys(), GROUP)
        self.assertEqual(successor["runtime-omarchy"]["receipt"], finals["runtime-omarchy"])
        for key in ("am", "runtime"):
            old = original[key]["reservation"]
            new = successor[key]["reservation"]
            self.assertEqual(new["previous"], old["version"])
            self.assertEqual(new["source"], self.server.sources[1] if key == "am" else old["source"])
            # A started build in any lane may still finish; only its finalizer writes the outcome.
            self.assertNotIn(outcome_ref(key, old["version"]).removeprefix("refs/tags/"), self.server.refs)
        pushes = [args for args in self.server.commands if args[1] == "push"]
        self.assertEqual(len(pushes), 2)
        self.assertIn("--atomic", pushes[-1])
        finals["am"] = self.finalize(successor["am"]["reservation"])
        self.assert_web_pushes_preserve_owner(successor["runtime"]["reservation"], finals)

    def owned(self, task):
        env = {"CD_COMPONENT": task["component"], "CD_TASKS": json.dumps([task]), "GH_TOKEN": "fixture",
               "GITHUB_RUN_ID": str(task["reservation"]["run_id"]), "GITHUB_RUN_ATTEMPT": "1"}
        with patch.object(ownership, "event_identity"):
            return ownership.owned(env, Path.cwd(), github=lambda _token: self.server)

    def test_same_source_claim_skips_once_its_started_peer_finalizes_and_the_next_push_resumes_it(self):
        for pushed in sorted(GROUP):
            with self.subTest(pushed=pushed):
                self.setUp()
                original = self.push(GROUP)
                successor = self.push({pushed})
                publisher = Reservations(self.server, "fixture", Path.cwd())
                finals = {}
                for key in sorted(GROUP - {pushed}):
                    old, new = original[key]["reservation"], successor[key]["reservation"]
                    # The group claims a fresh version of the same source the original is still building.
                    self.assertEqual((new["source"], new["previous"]), (old["source"], old["version"]))
                    self.assertTrue(publisher.require_current(new, build_started=False))
                    self.assertTrue(self.owned(successor[key]))
                    # Under the lane lock the original build finishes and finalizes first.
                    finals[key] = self.finalize(old)
                    # The source is published: the claim yields before building, and the
                    # ownership precheck skips its restores. A started build would fail instead.
                    with self.assertRaises(SourceAlreadyFinal):
                        publisher.require_current(new, build_started=False)
                    self.assertFalse(self.owned(successor[key]))
                    with self.assertRaisesRegex(Refusal, "different immutable release") as error:
                        publisher.require_current(new, build_started=True)
                    self.assertNotIsInstance(error.exception, ReservationSuperseded)
                    self.assertIn(pending_name(new), self.server.refs)
                finals[pushed] = self.finalize(successor[pushed]["reservation"])
                # Both runs have ended; the burned claims stay, and an unrelated push resumes the group.
                self.completed.update({801, 802})
                resumed = self.push({"web"})
                self.assertEqual(resumed.keys(), GROUP | {"web"})
                for key in GROUP:
                    self.assertEqual(resumed[key], {"component": key, "mode": "resume", "receipt": finals[key]})
                self.assertEqual(resumed["web"]["mode"], "publish")

    def test_same_source_skip_stays_red_when_a_newer_push_plans_around_it(self):
        for pushed in sorted(GROUP):
            with self.subTest(pushed=pushed):
                self.setUp()
                original = self.push(GROUP)
                successor = self.push({pushed})
                skipped = sorted(GROUP - {pushed})
                for key in skipped:
                    self.finalize(original[key]["reservation"])
                    self.assertFalse(self.owned(successor[key]))
                self.finalize(successor[pushed]["reservation"])
                # Run 802 is still running when the next push allocates: its burned
                # same-source claims look like an active owner, so it plans only web.
                self.completed.add(801)
                self.assertEqual(self.push({"web"}).keys(), {"web"})
                # So the skipping run must stay red: nothing else will deploy its group.
                report = report_fixtures.ComponentReportTests()
                env = report.same_source_skip(skipped[0])
                env.update({"CD_DISPOSITION_" + skipped[1].upper().replace("-", "_"): "source_already_final",
                            "CD_FINALIZED_" + skipped[1].upper().replace("-", "_"): ""})
                rows, verified = summarize(env)
                self.assertIs(verified, False)
                for key in skipped:
                    # The outcome columns are unchanged; the row then explains the skip.
                    self.assertIn(f"| {key} | source_already_final | skipped | skipped | unreported | "
                                  "source_already_final | operator | ", "\n".join(rows))
