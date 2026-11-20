import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, create_autospec, patch

import staging_cd_release_ownership as ownership
from staging_cd_common import Refusal
from staging_cd_receipts_tests import fixture_identity, fixture_receipt, fixture_runtime_receipt
from staging_cd_reservations import ReservationSuperseded, Reservations
from staging_cd_versions import DEPLOYABLES

LANES = sorted(DEPLOYABLES)


def reservation(component, **changes):
    value = {key: item for key, item in fixture_identity(component=component, **changes).items() if key != "build"}
    value["schema"] = "cma-version-reservation-v1"
    return value


class OwnershipTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.output = Path(self.temp.name) / "output"
        self.output.write_text("")
        self.env = {"CD_COMPONENT": "am", "GITHUB_RUN_ID": "100", "GITHUB_RUN_ATTEMPT": "1",
                    "GH_TOKEN": "synthetic", "GITHUB_OUTPUT": str(self.output)}

    def owned(self, tasks, require_current, component):
        env = dict(self.env, CD_COMPONENT=component, CD_TASKS=json.dumps(tasks))
        # Autospec enforces the real signature, including its required keyword.
        authority = create_autospec(Reservations, instance=True)
        authority.require_current.side_effect = require_current
        with patch.object(ownership, "event_identity"):
            result = ownership.owned(env, Path(self.temp.name), github=Mock(),
                                     reservations=Mock(return_value=authority))
        return result, authority

    def test_every_lane_keeps_a_current_build_and_skips_restores_once_superseded(self):
        for component in LANES:
            with self.subTest(component=component):
                task = {"component": component, "mode": "publish", "reservation": reservation(component)}
                result, authority = self.owned([task], lambda value, **_kwargs: True, component)
                self.assertTrue(result)
                authority.require_current.assert_called_once_with(task["reservation"], build_started=False)
                result, _ = self.owned([task], ReservationSuperseded(), component)
                self.assertFalse(result)

    def test_resumed_receipt_still_needs_the_job_in_every_lane(self):
        for component in LANES:
            with self.subTest(component=component):
                make = fixture_runtime_receipt if component.startswith("runtime") else fixture_receipt
                resume = {"component": component, "mode": "resume", "receipt": make(component=component)}
                result, authority = self.owned([resume], ReservationSuperseded(), component)
                self.assertTrue(result)
                authority.require_current.assert_not_called()

    def test_genuine_failures_foreign_owners_and_unknown_components_are_not_reported_as_superseded(self):
        for component in LANES:
            with self.subTest(component=component):
                task = {"component": component, "mode": "publish", "reservation": reservation(component)}
                with self.assertRaises(Refusal) as error:
                    self.owned([task], Refusal("Publication reservation is superseded and burned"), component)
                self.assertNotIsInstance(error.exception, ReservationSuperseded)
                foreign = {"component": component, "mode": "publish",
                           "reservation": reservation(component, run_id=99)}
                with self.assertRaises(Refusal):
                    self.owned([foreign], lambda value, **_kwargs: True, component)
        retired = {"component": "image-manager", "mode": "publish", "reservation": reservation("image-manager")}
        with self.assertRaisesRegex(Refusal, "component is unknown"):
            self.owned([retired], lambda value, **_kwargs: True, "image-manager")

    def test_main_reports_the_decision_as_a_step_output(self):
        for decision, expected in ((True, "false"), (False, "true")):
            with self.subTest(decision=decision):
                self.output.write_text("")
                with patch.dict("os.environ", self.env, clear=True), \
                        patch.object(ownership, "owned", return_value=decision), \
                        patch("sys.stdout"):
                    self.assertEqual(ownership.main(), 0)
                self.assertEqual(self.output.read_text(), f"superseded={expected}\n")


if __name__ == "__main__":
    unittest.main()
