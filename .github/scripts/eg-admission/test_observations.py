import json
import unittest
from pathlib import Path

from observations import collect, digest, parse_line
from observation_schema import PUBLIC_MAPPING, validate_event

ATTEMPT = (
    'eg_admission_attempt{attempt_sha256="'
    + "a" * 64
    + '" process_sha256="'
    + "b" * 64
    + '" admission_sha256="'
    + "c" * 64
    + '"}:'
)
REQUEST = 'cmaeg.request{request_id="10000000-0000-4000-8000-000000000001" route="/v1/callbacks/{channel_id}"}:'
POD = "d" * 64


def operation(stage, key):
    return f'eg_admission_operation{{stage="{stage}" operation_sha256="{key * 64}"}}:'


def line(
    event,
    spans=REQUEST + ATTEMPT,
    target="cmaeg::ingress::admission_trace",
    message="Admission observation",
    newline="\n",
):
    fields = " ".join(
        f"{key}={json.dumps(value, separators=(',', ':'))}"
        for key, value in event.items()
    )
    return (
        f"2026-11-14T00:00:00.000Z DEBUG {spans} {target}: {message} {fields}{newline}"
    )


def stage(boundary="end", classification="ok"):
    return {
        "event": "eg_admission_stage",
        "boundary": boundary,
        "class": classification,
        "elapsed_ms": 12,
    }


def record(**overrides):
    result = {
        "event": "eg_admission_record",
        "record_operation": "replica_write",
        "record_operation_sequence": 1,
        "transaction_attempt": 1,
        "phase": "commit",
        "io_class": "commit",
        "boundary": "error",
        "outcome": "error",
        "error_class": "unknown_commit",
        "label_class": "unknown_commit",
        "commit_class": "unknown",
        "elapsed_ms": 12,
        "mongo_labels_observed": True,
        "transient_transaction_label": True,
        "unknown_commit_label": True,
    }
    result.update(overrides)
    return result


class ObservationsTest(unittest.TestCase):
    def test_bound_rust_formatter_fixture_preserves_nested_spans(self):
        # Emitted with the application's pinned tracing-subscriber dependency.
        # These are synthetic formatter records, never application observations.
        raw = (
            Path(__file__)
            .with_name("fixtures")
            .joinpath("tracing-format.txt")
            .read_bytes()
        )
        packet = collect(raw, POD)
        self.assertEqual(packet["rejected_records"], 0)
        self.assertEqual(len(packet["observations"]), 3)
        self.assertEqual(
            packet["observations"][-1]["public_code"], "provider_unavailable"
        )
        for event in packet["observations"]:
            self.assertEqual(
                [item["stage"] for item in event["operations"]],
                ["root_readback", "root_lookup"],
            )

    def test_nested_operations_and_common_source_order_are_retained(self):
        nested = (
            REQUEST
            + ATTEMPT
            + operation("root_readback", "e")
            + operation("root_lookup", "f")
        )
        first = line(stage("start", "pending"), nested)
        second = line(stage(), nested, newline="\r\n")
        raw = ("unrelated private-canary\n" + first + second).encode()
        result = collect(raw, POD)
        self.assertEqual(result["input_sha256"], digest(raw))
        self.assertEqual(result["rejected_records"], 0)
        self.assertEqual(
            [row["source_ordinal"] for row in result["observations"]], [2, 3]
        )
        row = result["observations"][1]
        self.assertEqual(
            [value["stage"] for value in row["operations"]],
            ["root_readback", "root_lookup"],
        )
        self.assertEqual(
            row["request_sha256"], digest(b"10000000-0000-4000-8000-000000000001")
        )
        self.assertNotIn("private-canary", json.dumps(result))
        self.assertNotIn("route", json.dumps(result))

    def test_cancelled_attempt_without_http_parent_remains_uncertain(self):
        event = {
            "event": "eg_admission_attempt",
            "boundary": "cancelled",
            "class": "uncertain",
            "elapsed_ms": 2000,
        }
        row = parse_line(line(event, ATTEMPT))
        self.assertIsNone(row["request_sha256"])
        self.assertEqual(row["class"], "uncertain")
        self.assertEqual(row["operations"], [])

    def test_records_keep_both_labels_and_cannot_downgrade_unknown_commit(self):
        kwargs = {
            "spans": REQUEST + ATTEMPT + operation("root_write", "e"),
            "target": "cmaeg::records::mongo::replica_observation",
            "message": "replica persistence observation",
        }
        event = record()
        self.assertEqual(parse_line(line(event, **kwargs))["commit_class"], "unknown")
        for bad in [
            record(commit_class="aborted"),
            record(label_class="transient_transaction"),
            record(mongo_labels_observed=False),
            record(transaction_attempt=6),
            record(transaction_attempt=True),
            record(elapsed_ms=-1),
            record(error_class="private-error-canary"),
            record(io_class="submitted"),
            record(transient_transaction_label="true"),
            record(record_operation_sequence=0),
        ]:
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                parse_line(line(bad, **kwargs))
        bad = record(
            record_operation="admission_index",
            unknown_commit_label=False,
            error_class="transient_transaction",
            label_class="transient_transaction",
            commit_class="aborted",
        )
        with self.assertRaises(ValueError):
            parse_line(line(bad, **kwargs))

    def test_final_partial_numbers_and_injected_suffixes_are_rejected(self):
        valid = line(stage(), ATTEMPT + operation("root_lookup", "e"))
        variants = [
            valid.rstrip("\n"),
            valid.replace("elapsed_ms=12", "elapsed_ms=1").rstrip("\n"),
            valid.replace("\n", ' extra="private-canary"\n'),
            valid.replace('class="ok"', 'class="ok" class="ok"'),
            valid.replace("\n", " garbage\n"),
            valid.replace("DEBUG", "\x1b[0mDEBUG"),
            valid.replace('class="ok"', 'class="private-canary"'),
            valid.replace("elapsed_ms=12", "elapsed_ms=86400001"),
        ]
        for raw in variants:
            result = collect(raw.encode(), POD)
            self.assertEqual(result["observations"], [], raw)
            self.assertEqual(result["rejected_records"], 1)
            self.assertNotIn("private-canary", json.dumps(result))

    def test_operation_starts_ends_and_interruptions_keep_distinct_outcomes(self):
        for boundary, outcome, error, commit in [
            ("start", "pending", "none", "unconfirmed"),
            ("end", "ok", "none", "acknowledged"),
            ("end", "error", "unavailable", "unknown"),
            ("deadline", "deadline", "deadline", "unknown"),
            ("cancelled", "cancelled", "cancelled", "unknown"),
        ]:
            event = record(
                boundary=boundary,
                outcome=outcome,
                error_class=error,
                commit_class=commit,
                label_class="not_observed",
                mongo_labels_observed=False,
                transient_transaction_label=False,
                unknown_commit_label=False,
            )
            validate_event(event)
            for bad in [
                {**event, "outcome": "unexpected"},
                {**event, "commit_class": "aborted"},
                {**event, "io_class": "read"},
                {**event, "unknown_commit_label": True},
            ]:
                with self.assertRaises(ValueError):
                    validate_event(bad)
        for operation, commit in [
            ("replica_write", "aborted"),
            ("admission_index", "unknown"),
        ]:
            validate_event(
                record(
                    record_operation=operation,
                    error_class="transient_transaction",
                    label_class="transient_transaction",
                    commit_class=commit,
                    unknown_commit_label=False,
                )
            )
        validate_event(
            record(
                transient_transaction_label=False,
                unknown_commit_label=False,
                error_class="unlabelled_mongo",
                label_class="unlabelled_mongo",
            )
        )
        for invalid in [
            stage("cancelled", "ok"),
            stage("start", "ok"),
            stage("end", "pending"),
        ]:
            with self.assertRaises(ValueError):
                validate_event(invalid)

    def test_no_inference_from_wrong_missing_or_ambiguous_span_identity(self):
        event = stage()
        valid = ATTEMPT + operation("root_write", "e")
        for spans in [
            "",
            REQUEST,
            ATTEMPT,
            ATTEMPT + valid,
            REQUEST + REQUEST + valid,
            valid.replace("a" * 64, "A" * 64),
            valid.replace('stage="root_write"', 'stage="secret"'),
            REQUEST.replace(
                "10000000-0000-4000-8000-000000000001", "private-user-canary"
            )
            + valid,
        ]:
            with self.subTest(spans=spans), self.assertRaises(ValueError):
                parse_line(line(event, spans))

    def test_public_mapping_and_response_keep_actual_fixed_codes(self):
        for classification, (status, code, detail) in PUBLIC_MAPPING.items():
            event = {
                "event": "eg_admission_public_mapping",
                "class": classification,
                "public_status": status,
                "public_code": code,
                "public_detail": detail,
            }
            row = parse_line(
                line(
                    event,
                    target="cmaeg::ingress::routed::problem",
                    message="Admission error mapped to public refusal",
                )
            )
            self.assertEqual(row["public_code"], code)
            event["public_status"] = 202
            with self.assertRaises(ValueError):
                validate_event(event)
        event = {
            "event": "eg_admission_public_mapping",
            "class": "unavailable",
            "public_status": 503,
            "public_code": "provider_unavailable",
            "public_detail": "routed_admission_unavailable",
        }
        row = parse_line(
            line(
                event,
                spans=REQUEST,
                target="cmaeg::ingress::routed::problem",
                message="Admission error mapped to public refusal",
            )
        )
        self.assertIsNone(row["attempt_sha256"])
        self.assertIsNone(row["process_sha256"])
        self.assertIsNone(row["admission_sha256"])
        self.assertIsNotNone(row["request_sha256"])
        event = {
            "event": "eg_admission_response",
            "public_status": 503,
            "public_code": "provider_unavailable",
        }
        # ProblemCode Display is unquoted in the real producer.
        raw = line(event, message="Admission returned a refusal").replace(
            '"provider_unavailable"', "provider_unavailable"
        )
        self.assertEqual(parse_line(raw)["public_status"], 503)
        for bad in [
            {**event, "public_code": "private-canary"},
            {**event, "public_status": 202},
        ]:
            with self.assertRaises(ValueError):
                validate_event(bad)

    def test_malformed_framing_and_foreign_targets_cannot_supply_observations(self):
        valid = line(stage(), ATTEMPT + operation("root_write", "e"))
        variants = [
            valid.replace("DEBUG", "INFOextra"),
            valid.replace("}: ", "} "),
            valid.replace("cmaeg::ingress::admission_trace", "untrusted"),
            valid.replace("Admission observation", "private-canary"),
            valid.replace('event="eg_admission_stage"', 'event="unknown"'),
            valid.replace('class="ok"', 'class="ok"broken=1'),
            valid.replace('class="ok"', 'class="' + "a" * 1025 + '"'),
        ]
        for raw in variants:
            with self.subTest(raw=raw), self.assertRaises(ValueError):
                parse_line(raw)
        self.assertEqual(
            collect(b'INFO unrelated event="eg_admission_stage"\xff\n', POD)[
                "rejected_records"
            ],
            1,
        )
        with self.assertRaises(ValueError):
            collect(b"", "not-a-hash")
        with self.assertRaises(ValueError):
            collect("not-bytes", POD)


if __name__ == "__main__":
    unittest.main()
