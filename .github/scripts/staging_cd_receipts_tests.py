"""Release resume trusts complete immutable receipts, never mutable tags alone."""
import copy
import unittest

from staging_cd_common import Refusal
from image_scanner_policy import MANIFEST_SHA256
from staging_cd_receipts import (
    SUPPORTED_SCANS,
    annotation, parse_annotation, published_receipt, validate_identity, validate_receipt,
)
from staging_cd_versions import plan_version


def fixture_identity(**changes):
    previous, version = ("0.10.1", "0.10.2") if changes.get("component") == "runtime" else ("0.10.0", "0.10.1")
    return dict({"schema": "cma-image-release-v1", "component": "web", "version": version,
                 "previous": previous, "source": "a" * 40, "tree": "b" * 40, "pr": 42,
                 "bump": "patch", "run_id": 100, "run_attempt": 1, "build": "c" * 64}, **changes)


def fixture_receipt(**changes):
    from staging_cd_contract import GROUP, SCHEMA
    identity = fixture_identity(**changes)
    contract = ({"schema": SCHEMA, "source": identity["source"],
                 "runtime_contract": "3", "bootstrap": "1", "home_layout": "2"}
                if identity["component"] in GROUP else None)
    return published_receipt(identity, "sha256:" + "d" * 64, "e" * 64,
                             {"detected": 3, "blocking": 0, "advisory": 2, "reviewed": 1, "allowlisted": 0},
                             scanner_identity=("1.0.4", MANIFEST_SHA256), compatibility=contract)


def fixture_runtime_receipt(component="runtime", **changes):
    receipt = fixture_receipt(component=component, **changes)
    receipt["runtime_smoke"] = {"schema": "cma-runtime-smoke-v1", "source": receipt["source"],
        "digest": receipt["digest"], "archive": receipt["archive"], "evidence": "f" * 64,
        "scope": "local-contract-3-no-provider-or-model", "passed": True}
    return receipt


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

    def test_recovery_reads_reviewed_pairs_but_writer_requires_its_verified_identity(self):
        receipt = fixture_receipt()
        previous = ("c84f531cc76a09aca96ba5fa153d31a640e119fb27b9c0c73b3a9d538c94f9df",
                    "0d9e8ed1bd80b7e055107023fe73ce7c5cebb241c91667c07157d39c775b8aa3")
        for digest in previous:
            self.assertIn(("1.0.3", digest), SUPPORTED_SCANS)
        current = (receipt["scan"]["version"], receipt["scan"]["bundle"])
        self.assertEqual(current, ("1.0.4", MANIFEST_SHA256))
        self.assertIn(("1.0.3", "1d7bc6478a8e4c8aa11c4e7293403ce5128efde58dec3206cf2d219e896a9dce"), SUPPORTED_SCANS)
        self.assertIn(("1.0.4", "06e96df755ff3d8a60894bac93cfc9f80e713b44024b914b963f60ba00d7d2aa"), SUPPORTED_SCANS)
        for version, bundle in SUPPORTED_SCANS:
            old = dict(receipt, scan=dict(receipt["scan"], version=version, bundle=bundle))
            self.assertEqual(parse_annotation(annotation(old)), old)
            crossed = "1.0.3" if version == "1.0.4" else "1.0.4"
            with self.subTest(version=crossed, bundle=bundle), self.assertRaises(Refusal):
                validate_receipt(dict(old, scan=dict(old["scan"], version=crossed)))
        wrong = [*sorted(SUPPORTED_SCANS - {current}), ("1.0.3", MANIFEST_SHA256), ("1.0.4", "f" * 64)]
        for pair in wrong:
            with self.subTest(pair=pair), self.assertRaises(Refusal):
                published_receipt(receipt["identity"], receipt["digest"], receipt["archive"],
                                  receipt["scan"]["counts"], scanner_identity=pair)

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
