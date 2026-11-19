"""A later web push must leave the original run able to deploy Event Gateway."""
import fnmatch
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import Mock, patch

from staging_cd_allocate import allocate_queue
from staging_cd_allocation_fixture import AllocationServer
from staging_cd_common import Refusal
from staging_cd_component_admission import automatic_admission
from staging_cd_deploy_result_tests import fixture_result
from staging_cd_invoke import deploy, NotAdmitted
import staging_cd_invoke_tests as invoke_fixtures
from staging_cd_receipts import canonical
from staging_cd_receipts_tests import fixture_receipt, fixture_tag
from staging_cd_reservations import pending_name
from staging_cd_set import verify_set
from staging_cd_versions import DEPLOYABLES
from staging_cd_watermark import SupersededVersions, eligible_receipts, require_current_versions


class TakeoverDeployTests(unittest.TestCase):
    def exercise(self, *, timing, unreadable=False):
        server = AllocationServer(1, reserved=False)
        older, newer = server.sources
        receipts = {key: fixture_receipt(component=key, source=older) for key in ("eg", "web")}
        matrix = {"include": [{"component": key, "tasks": canonical([
            {"component": key, "mode": "resume", "receipt": receipt}])} for key, receipt in receipts.items()]}
        api, registry, reservations = Mock(), Mock(), Mock()
        api.tags.side_effect = lambda key=None: [fixture_tag(r) for k, r in receipts.items() if key in (k, None)]
        api.history, api.request = server.history, server.request
        registry.exists.return_value = True
        registry.manifest.return_value = (receipts["web"]["digest"], {})
        refs = {"refs/tags/" + r["tag"] for r in receipts.values()}

        def advertisement(args, **_kwargs):
            lines = ["1" * 40 + "\t" + ref for ref in refs if any(fnmatch.fnmatchcase(ref, p) for p in args)]
            return subprocess.CompletedProcess(args, 0, "\n".join(lines), "")

        def check(selected):
            require_current_versions(selected, runner=advertisement)

        api.require_current_versions.side_effect = check
        frozen, failures = automatic_admission(matrix, api, registry)
        self.assertEqual(failures, {})
        self.assertEqual(frozen["receipts"], receipts)
        changes = [{"source": older, "tree": "b" * 40, "deployables": sorted(DEPLOYABLES[k] for k in receipts)},
                   {"source": newer, "tree": "e" * 40, "deployables": [DEPLOYABLES["web"]]}]
        def next_merge():
            reservations.list.return_value = []
            result = allocate_queue({"event_source": newer, "head": newer, "changes": changes},
                                    api, reservations, 101, 1)
            tasks = {row["component"]: json.loads(row["tasks"]) for row in result["include"]}
            self.assertEqual(tasks.keys(), {"eg", "web"})
            self.assertEqual(tasks["eg"], [{"component": "eg", "mode": "resume", "receipt": receipts["eg"]}])
            web, = tasks["web"]
            self.assertEqual(web["mode"], "publish")
            reservation = web["reservation"]
            reservations.insert.assert_called_once_with(reservation)
            self.assertEqual(reservation["version"], "0.10.2")
            refs.add("refs/tags/" + pending_name(reservation))
        if timing == "before-lock":
            next_merge()
        verified = verify_set(frozen, api, registry, automatic=True)
        self.assertEqual(set(verified["receipts"]), {"eg"} if timing == "before-lock" else {"eg", "web"})
        if timing == "during-setup":
            next_merge()
        if unreadable:
            refs.remove("refs/tags/" + receipts["eg"]["tag"])
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            env = invoke_fixtures.InvokeTests().environment(root)
            env["CD_RELEASE_SET"] = canonical(verified)
            def controller(_script, path, *_args):
                result = fixture_result(path)
                self.assertEqual(set(result["components"]), {"eg"})
                self.assertEqual(result["components"]["eg"]["source_sha"], older)
                return 0
            runner = Mock(side_effect=controller)
            with patch("staging_cd_invoke.require_current_versions", side_effect=check):
                args = (env, Mock(), lambda _: root / "controller.py", runner)
                def policy(_script, keys, **_kwargs):
                    return sorted(keys)
                if unreadable:
                    with self.assertRaises(NotAdmitted):
                        deploy(*args, policy_reader=policy)
                    runner.assert_not_called()
                else:
                    self.assertEqual(deploy(*args, policy_reader=policy), 0)
                    runner.assert_called_once()

    def test_successor_after_admission_preserves_the_other_components_controller(self):
        self.exercise(timing="before-lock")

    def test_successor_after_lock_verification_preserves_the_other_components_controller(self):
        self.exercise(timing="during-setup")

    def test_malformed_or_missing_authority_prevents_even_partial_controller_launch(self):
        self.exercise(timing="during-setup", unreadable=True)

    def test_only_contract_transitions_hold_all_remaining_group_members(self):
        receipts = {key: fixture_receipt(component=key) for key in ("am", "runtime", "eg")}
        check = Mock(side_effect=SupersededVersions({"am"}))
        policy = Mock()
        for transition, expected in [(True, {"eg"}), (False, {"runtime", "eg"})]:
            policy.return_value.transition.return_value = transition
            self.assertEqual(set(eligible_receipts(receipts, check, policy)), expected)
        policy.return_value.transition.side_effect = Refusal("unavailable history")
        with self.assertRaisesRegex(Refusal, "unavailable history"):
            eligible_receipts(receipts, check, policy)
