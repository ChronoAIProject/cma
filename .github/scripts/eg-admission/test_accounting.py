import copy
import unittest

from accounting import account

EVENT, KEY, REQUEST, CASE = (c * 64 for c in "abcd")


def inputs():
    target = {"complete": True, "idempotency_sha256": KEY}
    durable = {
        "event_sha256": EVENT,
        "root_present": True,
        "admission_sha256": "0" * 64,
        "targets": [target],
    }
    request = {
        "request_sha256": REQUEST,
        "status": 202,
        "accepted_body": True,
        "body_complete": True,
    }
    return {
        "registry": [{"event_sha256": EVENT, "expect": "success"}],
        "cases": [
            {
                "schema": "eg_fixture_case_v1",
                "case_sha256": CASE,
                "event_sha256": EVENT,
                "durable": copy.deepcopy(durable),
                "requests": [request],
            }
        ],
        "final": {
            "schema": "eg_fixture_reconciliation_v1",
            "events": [durable],
            "ledger": {
                "closed": True,
                "poisoned": False,
                "journal_attached": True,
                "rejected_requests": 0,
                "provider_operations": [
                    {"event_sha256": EVENT, "idempotency_sha256": KEY}
                ],
                "sink_effects": [
                    {"event_sha256": EVENT, "text_matches": True, "after_close": False}
                ],
            },
        },
        "captures": [
            {
                "rejected_records": 0,
                "observations": [
                    {
                        "event": "eg_admission_attempt",
                        "request_sha256": REQUEST,
                        "admission_sha256": "0" * 64,
                        "process_sha256": "1" * 64,
                        "attempt_sha256": "2" * 64,
                        "boundary": boundary,
                    }
                    for boundary in ("start", "end")
                ],
            }
        ],
        "finality": {
            "receipt_sha256": "e" * 64,
            "all_gateway_processes_exited": True,
            "no_late_effect_barrier": True,
            "capture_complete": True,
        },
    }


class AccountingTest(unittest.TestCase):
    def test_two_replay_requests_are_one_event_and_one_effect(self):
        data = inputs()
        other = copy.deepcopy(data["cases"][0]["requests"][0])
        other["request_sha256"] = "f" * 64
        data["cases"][0]["requests"].append(other)
        data["captures"][0]["observations"] += [
            {**row, "request_sha256": "f" * 64, "attempt_sha256": "3" * 64}
            for row in data["captures"][0]["observations"]
        ]
        result = account(**data)
        self.assertEqual(result["counts"]["http_attempts"], 2)
        self.assertEqual(result["counts"]["submitted_events"], 1)
        self.assertEqual(result["counts"]["direct_terminal"], 1)
        self.assertEqual(result["counts"]["reconciled_terminal"], 0)
        self.assertEqual(result["counts"]["confirmed_effects"], 1)
        self.assertEqual(result["evidence_status"], "COMPLETE")
        self.assertEqual(
            result["application_acceptance"], "REQUIRES_INDEPENDENT_REVIEW"
        )

    def test_refused_http_with_committed_effect_is_reconciled_but_healthy_contract_fails(
        self,
    ):
        data = inputs()
        data["cases"][0]["requests"][0]["status"] = 503
        result = account(**data)
        self.assertEqual(result["counts"]["reconciled_terminal"], 1)
        self.assertEqual(result["counts"]["direct_terminal"], 0)
        self.assertEqual(result["counts"]["proven_unadmitted"], 0)
        self.assertEqual(result["evidence_status"], "INCOMPLETE")

    def test_missing_http_case_or_missing_final_snapshot_never_shrinks_denominator(
        self,
    ):
        for field in ("cases", "events"):
            data = inputs()
            if field == "cases":
                data["cases"] = []
            else:
                data["final"]["events"] = []
            result = account(**data)
            self.assertEqual(result["counts"]["submitted_events"], 1)
            self.assertEqual(result["evidence_status"], "INCOMPLETE")

    def test_unknown_or_unresolved_work_cannot_be_removed_by_reconciliation_attempt(
        self,
    ):
        data = inputs()
        data["final"]["events"][0]["targets"][0]["complete"] = False
        result = account(**data)
        self.assertEqual(result["counts"]["unresolved"], 1)
        self.assertEqual(result["counts"]["reconciled_terminal"], 0)
        data["final"]["events"][0] = {
            "event_sha256": EVENT,
            "oracle_error": "read_failed",
        }
        self.assertEqual(account(**data)["counts"]["admission_uncertain"], 1)

    def test_duplicate_missing_stale_and_unjoined_effects_each_hold_closure(self):
        for variant in (
            "duplicate_sink",
            "duplicate_provider",
            "missing",
            "late",
            "wrong_text",
            "wrong_key",
        ):
            data = inputs()
            ledger = data["final"]["ledger"]
            if variant == "duplicate_sink":
                ledger["sink_effects"] *= 2
            elif variant == "duplicate_provider":
                ledger["provider_operations"] *= 2
            elif variant == "missing":
                ledger["sink_effects"] = []
            elif variant == "late":
                ledger["sink_effects"][0]["after_close"] = True
            elif variant == "wrong_text":
                ledger["sink_effects"][0]["text_matches"] = False
            else:
                ledger["provider_operations"][0]["idempotency_sha256"] = "0" * 64
            with self.subTest(variant=variant):
                result = account(**data)
                self.assertEqual(result["evidence_status"], "INCOMPLETE")
                self.assertEqual(result["counts"]["confirmed_effects"], 0)
                self.assertEqual(result["counts"]["unresolved"], 1)

    def test_finality_log_or_journal_gaps_prevent_terminal_acceptance(self):
        for variant in (
            "all_gateway_processes_exited",
            "no_late_effect_barrier",
            "capture_complete",
            "rejected",
            "poisoned",
            "not_closed",
            "no_journal",
        ):
            data = inputs()
            if variant in data["finality"]:
                data["finality"][variant] = False
            elif variant == "rejected":
                data["captures"][0]["rejected_records"] = 1
            elif variant == "poisoned":
                data["final"]["ledger"]["poisoned"] = True
            elif variant == "not_closed":
                data["final"]["ledger"]["closed"] = False
            else:
                data["final"]["ledger"]["journal_attached"] = False
            result = account(**data)
            self.assertEqual(result["evidence_status"], "INCOMPLETE")
            self.assertEqual(result["counts"]["unresolved"], 1)

    def refusal(self):
        data = inputs()
        data["registry"][0]["expect"] = "pre_submit_refusal"
        data["cases"][0]["requests"][0]["status"] = 503
        data["final"]["events"][0] = {
            "event_sha256": EVENT,
            "root_present": False,
            "admission_sha256": "0" * 64,
            "targets": [],
        }
        data["final"]["ledger"]["provider_operations"] = []
        data["final"]["ledger"]["sink_effects"] = []
        parent = {
            "request_sha256": REQUEST,
            "attempt_sha256": "2" * 64,
            "process_sha256": "1" * 64,
            "operations": [{"stage": "root_write"}],
        }
        data["captures"][0]["observations"] += [
            {**parent, "event": "eg_admission_stage", "boundary": "start"},
            {
                **parent,
                "event": "eg_fixture_record_fault",
                "boundary": "before_submission",
                "commit_class": "not_submitted",
            },
        ]
        return data

    def test_absence_and_503_need_actual_pre_submit_join_and_finality(self):
        data = self.refusal()
        result = account(**data)
        self.assertEqual(result["counts"]["proven_unadmitted"], 1)
        self.assertEqual(result["evidence_status"], "COMPLETE")
        data["captures"][0]["observations"] = []
        result = account(**data)
        self.assertEqual(result["counts"]["admission_uncertain"], 1)
        self.assertEqual(result["evidence_status"], "INCOMPLETE")

    def test_additional_write_phase_or_unmarked_attempt_defeats_no_submission_proof(
        self,
    ):
        for variant in ("write", "attempt", "wrong_request", "timeout"):
            data = self.refusal()
            rows = data["captures"][0]["observations"]
            if variant == "write":
                rows.append(
                    {
                        **rows[2],
                        "event": "eg_admission_record",
                        "phase": "record_insert",
                    }
                )
            elif variant == "attempt":
                rows.append({**rows[2], "attempt_sha256": "1" * 64})
            elif variant == "wrong_request":
                rows[3]["request_sha256"] = "1" * 64
            else:
                del data["cases"][0]["requests"][0]["status"]
            with self.subTest(variant=variant):
                self.assertEqual(account(**data)["counts"]["admission_uncertain"], 1)

    def test_duplicate_receipts_unregistered_effects_and_invalid_flags_are_rejected(
        self,
    ):
        for variant in ("registry", "case", "attempt", "event", "effect", "flag"):
            data = inputs()
            if variant == "registry":
                data["registry"] *= 2
            elif variant == "case":
                data["cases"] *= 2
            elif variant == "attempt":
                data["cases"][0]["requests"] *= 2
            elif variant == "event":
                data["final"]["events"] *= 2
            elif variant == "effect":
                data["final"]["ledger"]["sink_effects"][0]["event_sha256"] = "1" * 64
            else:
                data["finality"]["capture_complete"] = 1
            with self.subTest(variant=variant), self.assertRaises(ValueError):
                account(**data)


if __name__ == "__main__":
    unittest.main()
