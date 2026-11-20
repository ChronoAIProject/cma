"""Unrelated pushes must leave the current contract publisher able to finish."""
import hashlib
import json
from pathlib import Path
import unittest

from staging_cd_allocate import allocate_queue
from staging_cd_allocation_fixture import AllocationServer
from staging_cd_allocation_io import AllocationIO
from staging_cd_allocation_snapshot import AllocationSnapshot, SnapshotAPI
from staging_cd_common import Refusal
from staging_cd_compatibility import GROUP
from staging_cd_history import DevelopHistory
from staging_cd_receipts import annotation
from staging_cd_receipts_tests import fixture_receipt, fixture_runtime_receipt
from staging_cd_reservations import ReservationSuperseded, Reservations, outcome_ref, pending_name
from staging_cd_versions import DEPLOYABLES


class TransitionOwnerTests(unittest.TestCase):
    def setUp(self):
        self.server = AllocationServer(5, reserved=False)
        self.changes = []
        request = self.server.request

        def read(path, **kwargs):
            response = request(path, **kwargs)
            if path.startswith("actions/runs/"):
                response["status"] = "in_progress"
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

    def test_same_source_group_claim_refuses_before_building_once_the_started_peer_finalizes(self):
        original = self.push(GROUP)
        successor = self.push({"am"})
        publisher = Reservations(self.server, "fixture", Path.cwd())
        for key in sorted(GROUP - {"am"}):
            old, new = original[key]["reservation"], successor[key]["reservation"]
            # The group claims a fresh version of the same source the original is still building.
            self.assertEqual((new["source"], new["previous"]), (old["source"], old["version"]))
            self.assertTrue(publisher.require_current(new, build_started=False))
            # Under the lane lock the original build finishes and finalizes first.
            final = self.finalize(old)
            for started in (False, True):
                with self.subTest(component=key, started=started), \
                        self.assertRaisesRegex(Refusal, "different immutable release") as error:
                    publisher.require_current(new, build_started=started)
                # One final per source: the claim can never finalize, so it fails rather than rebuild.
                self.assertNotIsInstance(error.exception, ReservationSuperseded)
            self.assertEqual([tag["receipt"] for tag in self.server.tags(key)], [final])
            self.assertIn(pending_name(new), self.server.refs)
