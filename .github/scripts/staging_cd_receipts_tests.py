"""Release resume trusts complete immutable receipts, never mutable tags alone."""
import copy
import unittest

from staging_cd_common import Refusal
from staging_cd_receipts import (
    annotation, parse_annotation, published_receipt, validate_identity, validate_receipt,
)
from staging_cd_versions import plan_version


def fixture_identity(**changes):
    return dict({"schema": "cma-image-release-v1", "component": "web", "version": "0.10.1",
                 "previous": "0.10.0", "source": "a" * 40, "tree": "b" * 40, "pr": 42,
                 "bump": "patch", "run_id": 100, "run_attempt": 1, "build": "c" * 64}, **changes)


def fixture_receipt(**changes):
    return published_receipt(fixture_identity(**changes), "sha256:" + "d" * 64, "e" * 64,
                             {"detected": 3, "blocking": 0, "advisory": 2, "reviewed": 1, "allowlisted": 0})


def fixture_tag(receipt):
    return {"name": receipt["tag"], "annotation_name": receipt["tag"], "type": "tag",
            "target_type": "commit", "target": receipt["source"], "receipt": receipt}


class ReceiptTests(unittest.TestCase):
    def test_same_merge_resumes_without_bump_even_when_label_changed(self):
        receipt = fixture_receipt()
        plan = plan_version("web", "major", [fixture_tag(receipt)], receipt["source"])
        self.assertEqual(plan.version, "0.10.1")
        self.assertEqual(plan.receipt, receipt)
        next_plan = plan_version("web", "patch", [fixture_tag(receipt)], "f" * 40)
        self.assertEqual(next_plan.version, "0.10.2")
        self.assertIsNone(next_plan.receipt)

    def test_receipt_roundtrip_binds_scan_and_exact_registry_identity(self):
        receipt = fixture_receipt()
        self.assertEqual(parse_annotation(annotation(receipt)), receipt)
        for key, value in (("tag", "cma-web/v99.0.0"), ("repository", "ghcr.io/example/other"),
                           ("source", "f" * 40), ("digest", "latest"), ("archive", "bad")):
            with self.subTest(key=key), self.assertRaises(Refusal):
                validate_receipt(dict(receipt, **{key: value}))
        for key, value in (("complete", False), ("version", "1.0.1"), ("bundle", "f" * 64), ("mode", "all")):
            with self.subTest(key=key), self.assertRaises(Refusal):
                validate_receipt(dict(receipt, scan=dict(receipt["scan"], **{key: value})))
        for key, value in (("blocking", 1), ("detected", 0), ("advisory", True), ("allowlisted", 1)):
            scan = copy.deepcopy(receipt["scan"])
            scan["counts"][key] = value
            with self.subTest(key=key), self.assertRaises(Refusal):
                validate_receipt(dict(receipt, scan=scan))

    def test_malformed_or_conflicting_receipts_cannot_allocate_another_version(self):
        for changes in ({"schema": "other"}, {"version": "0.11.0"}, {"pr": True},
                        {"run_id": 0}, {"build": "missing"}, {"extra": "private-data"}):
            with self.subTest(changes=changes), self.assertRaises(Refusal):
                validate_identity(fixture_identity(**changes))
        first = fixture_receipt()
        second = fixture_receipt(version="0.11.0", bump="minor")
        with self.assertRaises(Refusal):
            plan_version("web", "patch", [fixture_tag(first), fixture_tag(second)], first["source"])
        for message in ("{}", "not JSON", annotation(first).replace('"schema":', '"schema":"duplicate","schema":', 1)):
            with self.subTest(message=message[:10]), self.assertRaises(ValueError):
                parse_annotation(message)


if __name__ == "__main__":
    unittest.main()
