import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

import staging_cd_am_ownership as ownership
from staging_cd_common import Refusal
from staging_cd_receipts_tests import fixture_identity, fixture_receipt
from staging_cd_reservations import ReservationSuperseded


def reservation(**changes):
    value = {key: item for key, item in fixture_identity(component="am", **changes).items() if key != "build"}
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

    def owned(self, tasks, require_current, component="am"):
        env = dict(self.env, CD_COMPONENT=component, CD_TASKS=json.dumps(tasks))
        authority = Mock()
        authority.require_current.side_effect = require_current
        with patch.object(ownership, "event_identity"):
            result = ownership.owned(env, Path(self.temp.name), github=Mock(),
                                     reservations=Mock(return_value=authority))
        return result, authority

    def test_current_reservation_keeps_the_build_and_superseded_one_skips_restores(self):
        task = {"component": "am", "mode": "publish", "reservation": reservation()}
        result, authority = self.owned([task], lambda value: True)
        self.assertTrue(result)
        authority.require_current.assert_called_once_with(task["reservation"])
        result, _ = self.owned([task], ReservationSuperseded())
        self.assertFalse(result)

    def test_resumed_receipt_or_any_current_reservation_still_needs_the_job(self):
        receipt = fixture_receipt(component="am")
        resume = {"component": "am", "mode": "resume", "receipt": receipt}
        result, authority = self.owned([resume], ReservationSuperseded())
        self.assertTrue(result)
        authority.require_current.assert_not_called()

    def test_genuine_failures_and_other_components_are_not_reported_as_superseded(self):
        task = {"component": "am", "mode": "publish", "reservation": reservation()}
        with self.assertRaises(Refusal):
            self.owned([task], Refusal("Publication reservation is superseded and burned"))
        web = {"component": "web", "mode": "publish",
               "reservation": dict(reservation(), component="web")}
        with self.assertRaises(Refusal):
            self.owned([web], lambda value: True, component="web")
        foreign = {"component": "am", "mode": "publish", "reservation": reservation(run_id=99)}
        with self.assertRaises(Refusal):
            self.owned([foreign], lambda value: True)

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
