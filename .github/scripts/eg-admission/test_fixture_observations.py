"""Fault provenance is synthetic, scoped to an observed root write, and closed."""

import unittest

from observations import collect
from test_observations import ATTEMPT, POD, REQUEST, line, operation

TARGET = "cmaeg::records::mongo::replicas::diagnostic_fault"
MESSAGE = "Diagnostic record fault fired"


def fault(**overrides):
    value = {
        "event": "eg_fixture_record_fault",
        "fixture_sha256": "e" * 64,
        "case_sha256": "f" * 64,
        "database_sha256": "d" * 64,
        "admission_sha256": "c" * 64,
        "action": "post_commit_unknown",
        "boundary": "after_commit_ack",
        "commit_class": "acknowledged",
    }
    value.update(overrides)
    return value


def capture(event, spans=REQUEST + ATTEMPT + operation("root_write", "e"), **kwargs):
    options = {"target": TARGET, "message": MESSAGE, **kwargs}
    return collect(line(event, spans, **options).encode(), POD)


class FixtureObservationsTest(unittest.TestCase):
    def test_exact_fired_boundaries_remain_distinct_from_record_outcomes(self):
        for action, boundary, commit in (
            ("pre_submit_refusal", "before_submission", "not_submitted"),
            ("pre_submit_stall", "before_submission", "not_submitted"),
            ("post_commit_unknown", "after_commit_ack", "acknowledged"),
        ):
            result = capture(
                fault(action=action, boundary=boundary, commit_class=commit)
            )
            self.assertEqual(result["rejected_records"], 0)
            (row,) = result["observations"]
            self.assertEqual(row["event"], "eg_fixture_record_fault")
            self.assertEqual(row["action"], action)
            self.assertEqual(row["operations"][-1]["stage"], "root_write")
            self.assertNotIn("provider_effect", row)
            self.assertNotIn("outcome", row)
        native = capture(fault(), ATTEMPT + operation("root_write", "e"))
        self.assertIsNone(native["observations"][0]["request_sha256"])

    def test_invalid_fault_provenance_is_counted_and_never_retained(self):
        variants = [
            {"action": "unknown"},
            {"boundary": "before_submission"},
            {"commit_class": "unknown"},
            {"case_sha256": "PRIVATE_CANARY"},
            {"database_sha256": "d" * 65},
            {"fixture_sha256": "E" * 64},
            {"admission_sha256": "b" * 64},
            {"payload": "PRIVATE_CANARY"},
            {"event": "eg_fixture_unrecognized"},
        ]
        for override in variants:
            with self.subTest(override=override):
                result = capture(fault(**override))
                self.assertEqual(result["rejected_records"], 1)
                self.assertEqual(result["observations"], [])
                self.assertNotIn("PRIVATE_CANARY", str(result))
        absent = fault()
        del absent["fixture_sha256"]
        self.assertEqual(capture(absent)["rejected_records"], 1)

    def test_root_parent_and_target_cannot_be_invented_or_reversed(self):
        for spans in (
            "",
            REQUEST,
            ATTEMPT,
            ATTEMPT + operation("root_readback", "e"),
            operation("root_write", "e") + ATTEMPT,
            ATTEMPT + REQUEST + operation("root_write", "e"),
        ):
            result = capture(fault(), spans)
            self.assertEqual(result["observations"], [])
            self.assertEqual(result["rejected_records"], 1)
        mismatch = capture(
            fault(),
            target="cmaeg::ingress::admission_trace",
            message="Admission observation",
        )
        self.assertEqual(mismatch["rejected_records"], 1)
        self.assertEqual(mismatch["observations"], [])

    def test_truncation_and_unknown_fields_do_not_disappear_from_denominator(self):
        raw = line(
            fault(),
            ATTEMPT + operation("root_write", "e"),
            target=TARGET,
            message=MESSAGE,
        )
        result = collect(raw[:-1].encode(), POD)
        self.assertEqual(result["rejected_records"], 1)
        self.assertEqual(result["observations"], [])
        wrong_message = capture(fault(), message="not the actual source branch")
        self.assertEqual(wrong_message["rejected_records"], 1)


if __name__ == "__main__":
    unittest.main()
