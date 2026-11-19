"""Independent releases retain the same admissible set and compatibility checks."""
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

from staging_cd_common import Refusal
from staging_cd_compatibility import GROUP
from staging_cd_receipts import canonical
from staging_cd_receipts_tests import fixture_receipt, fixture_runtime_receipt
from staging_cd_routes import check_lane, main, routes


def matrix(components):
    entries = []
    for k in components:
        make = fixture_runtime_receipt if k.startswith("runtime") else fixture_receipt
        r = make(component=k)
        entries.append({"component": k, "tasks": canonical([{"component": k, "mode": "resume", "receipt": r}])})
    return {"include": entries}


class RoutingTests(unittest.TestCase):
    def test_stable_contracts_keep_all_five_publications_independent(self):
        m = matrix(["web", "eg", *sorted(GROUP)])
        policy = Mock()
        policy.transition_sources.return_value = []
        policy.signature.return_value = ("same",)
        lanes, batches = routes(m, Mock(return_value=policy))
        self.assertEqual(lanes, {k: k for k in lanes})
        self.assertEqual([batches[e["component"]]["include"] for e in m["include"]], [[e] for e in m["include"]])

    def test_transitions_or_unreadable_history_only_join_contract_peers(self):
        m = matrix(["web", "am", "runtime"])
        for transition in (True, OSError("unavailable")):
            policy = Mock()
            if transition is True:
                policy.transition_sources.return_value = ["a" * 40]
            else:
                policy.transition_sources.side_effect = transition
            lanes, batches = routes(m, Mock(return_value=policy))
            self.assertEqual(lanes, {"web": "web", "am": "contract", "runtime": "contract"})
            self.assertEqual(batches["contract"]["include"], m["include"][1:])
            self.assertEqual(batches["web"]["include"], m["include"][:1])

    def test_incomplete_or_conflicting_transition_markers_cannot_be_routed(self):
        selected = matrix(sorted(GROUP))
        for entry in selected["include"]:
            entry["contract_transition"] = "a" * 40
        member = selected["include"][0]
        member["contract_transition"] = "b" * 40
        with self.assertRaisesRegex(Refusal, "marker coverage differs"):
            routes(selected, Mock())
        del member["contract_transition"]
        with self.assertRaisesRegex(Refusal, "marker coverage differs"):
            routes(selected, Mock())

    def test_stable_but_incompatible_selected_sources_go_to_joint_admission(self):
        m = matrix(["am", "runtime"])
        r = fixture_receipt(component="am", source="f" * 40)
        m["include"][0]["tasks"] = canonical([{"component": "am", "mode": "resume", "receipt": r}])
        policy = Mock()
        policy.transition_sources.return_value = []
        policy.signature.side_effect = lambda source: source
        self.assertEqual(set(routes(m, Mock(return_value=policy))[1]), {"contract"})

    def test_transition_retry_can_resume_exact_peers_only_in_joint_lane(self):
        m = matrix(["am"])
        release = {"receipts": dict.fromkeys(GROUP)}
        check_lane("contract", m, release, {})
        check_lane("am", m, None, {"am": "publication-unqualified"})
        for name, value in (("am", release), ("web", release), ("contract", {"receipts": {"web": 1}}),
                            ("am", {"receipts": {}})):
            with self.assertRaises(Refusal):
                check_lane(name, m, value, {})

    def test_workflow_output_round_trip_and_invalid_matrix(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory, "outputs")
            m = matrix(["web"])
            with patch.dict(os.environ, {"GITHUB_OUTPUT": str(path), "CD_PUBLICATION_MATRIX": canonical(m)}):
                self.assertEqual(main(), 0)
                data = {k: json.loads(v) for k, v in (line.split("=", 1) for line in path.read_text().splitlines())}
                self.assertEqual(data["batches"]["web"], m)
                self.assertEqual(data["publications"]["web"], m["include"][0]["tasks"])
                self.assertEqual(data["lanes"], {"web": "web"})
                path.unlink()
                with patch.dict(os.environ, {"CD_PUBLICATION_MATRIX": '{"include":[null]}'}):
                    self.assertNotEqual(main(), 0)
                self.assertFalse(path.exists())
