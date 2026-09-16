import copy
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from authority import SPEC_LABEL, fingerprint, identity, new_plan, record_created
from resume import resume_creation


class ResumeTests(unittest.TestCase):
    def setUp(self):
        self.plan = new_plan("fixture-context", "fixture-owner")
        self.desired = {"apiVersion": "v1", "kind": "Namespace", "metadata": {
            "name": self.plan["namespace"], "labels": {"cma/continuity-run": self.plan["run"]},
        }}
        self.observed = copy.deepcopy(self.desired)
        self.observed["metadata"].update(uid="original", resourceVersion="1")
        self.observed["metadata"]["labels"][SPEC_LABEL] = fingerprint(self.desired)[:63]

    def test_unknown_create_is_adopted_only_for_the_exact_request(self):
        with tempfile.TemporaryDirectory() as directory, patch("resume.get_resource", return_value=self.observed), patch("resume.create_resource") as create:
            self.write_intent(directory)
            resume_creation(Path(directory) / "plan.json", self.plan, [self.desired])
            create.assert_not_called()
            self.assertEqual(self.plan["namespace_uid"], "original")
            self.assertEqual(len(self.plan["resources"]), 1)

    def test_a_name_collision_with_different_spec_does_not_authorize_adoption(self):
        self.observed["metadata"]["labels"][SPEC_LABEL] = "different"
        with tempfile.TemporaryDirectory() as directory, patch("resume.get_resource", return_value=self.observed), patch("resume.create_resource") as create:
            self.write_intent(directory)
            with self.assertRaisesRegex(ValueError, "ambiguous_resource_spec_mismatch"):
                resume_creation(Path(directory) / "plan.json", self.plan, [self.desired])
            create.assert_not_called()
            self.assertEqual(self.plan["resources"], [])

    def write_intent(self, directory):
        path = Path(directory) / ("intent-namespace-" + self.plan["namespace"] + ".json")
        path.write_text(json.dumps({"identity": identity(self.desired), "spec_hash": fingerprint(self.desired)[:63]}))
        return path

    def test_a_matching_label_without_our_create_intent_is_not_authority(self):
        with tempfile.TemporaryDirectory() as directory, patch("resume.get_resource", return_value=self.observed):
            with self.assertRaisesRegex(ValueError, "unrecorded_create_intent"):
                resume_creation(Path(directory) / "plan.json", self.plan, [self.desired])
            path = self.write_intent(directory)
            intent = json.loads(path.read_text())
            intent["spec_hash"] = "different-request"
            path.write_text(json.dumps(intent))
            with self.assertRaisesRegex(ValueError, "create_intent_mismatch"):
                resume_creation(Path(directory) / "plan.json", self.plan, [self.desired])
            self.assertEqual(self.plan["resources"], [])

    def test_a_missing_journaled_resource_is_not_recreated(self):
        record_created(self.plan, self.observed)
        with patch("resume.get_resource", return_value=None), patch("resume.create_resource") as create:
            with self.assertRaisesRegex(ValueError, "journaled_resource_disappeared"):
                resume_creation(Path("unused"), self.plan, [self.desired])
            create.assert_not_called()


if __name__ == "__main__":
    unittest.main()
