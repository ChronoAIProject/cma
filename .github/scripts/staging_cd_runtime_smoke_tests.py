"""Runtime promotion requires a canary of the same scanned immutable archive."""
import copy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

from staging_cd_common import Refusal
from staging_cd_publish import publish
from staging_cd_publish_tests import PublicationTests
from staging_cd_receipts import validate_receipt
from staging_cd_receipts_tests import fixture_identity, fixture_runtime_receipt
from staging_cd_runtime_smoke import qualify, validate_smoke
from staging_cd_set import release_set


class RuntimeSmokeTests(unittest.TestCase):
    def fixture(self, directory, *, failure=None):
        root = Path(directory)
        archive = root / "image.oci.tar"
        archive.write_bytes(b"fixture")
        calls = []
        def run(args, **kwargs):
            calls.append((args, kwargs))
            if args[:3] == ["skopeo", "inspect", "--raw"]:
                return json.dumps({"config": {"digest": "sha256:" + "a" * 64}}).encode()
            if args[:3] == ["docker", "image", "inspect"]:
                return json.dumps([{"Id": "sha256:" + ("b" if failure == "config" else "a") * 64}]).encode()
            if args[0] == "node":
                if failure == "run":
                    raise Refusal("fixture_failure")
                evidence = {"result": "fail" if failure == "result" else "pass", "commit": "a" * 40,
                            "cleanup": {"containers": 1 if failure == "cleanup" else 0, "volumes": 0}}
                Path(args[-1]).write_text(json.dumps(evidence))
            return b""
        return root, archive, calls, run

    def test_same_archive_config_source_and_cleanup_are_bound(self):
        for component in ("runtime", "runtime-omarchy"):
            with tempfile.TemporaryDirectory() as directory:
                root, archive, calls, run = self.fixture(directory)
                identity = fixture_identity(component=component)
                proof = qualify(archive, identity, "sha256:" + "d" * 64, "e" * 64, root, run, source=root)
                self.assertTrue(proof["passed"])
                self.assertEqual(proof["source"], identity["source"])
                self.assertEqual(proof["digest"], "sha256:" + "d" * 64)
                self.assertEqual(proof["archive"], "e" * 64)
                self.assertEqual(calls[-1][0][:3], ["docker", "image", "rm"])
                self.assertEqual(calls[-1][0][-1], calls[0][0][-1].removeprefix("docker-daemon:"))
                self.assertEqual(Path(calls[3][0][1]).name, "runtime-image-evidence.mjs")
                self.assertEqual(calls[3][1]["env"]["CMA_RUNTIME_EVIDENCE_DOCKERFILE"],
                                 str(root / "api/crates/layer4-runtime/Dockerfile"))

    def test_failures_never_produce_proof_and_remove_only_owned_tag(self):
        for failure in ("config", "run", "result", "cleanup"):
            with tempfile.TemporaryDirectory() as directory:
                root, archive, calls, run = self.fixture(directory, failure=failure)
                with self.assertRaises(Refusal):
                    qualify(archive, fixture_identity(component="runtime"), "sha256:" + "d" * 64,
                            "e" * 64, root, run)
                self.assertEqual(calls[-1][0][:3], ["docker", "image", "rm"])
                self.assertNotIn("prune", str(calls))

    def test_timeout_cleanup_reconciles_only_campaign_resources(self):
        with tempfile.TemporaryDirectory() as directory:
            root, archive, calls, run = self.fixture(directory, failure="run")
            seen = set()
            def abandoned(args, **kwargs):
                if args[:2] == ["docker", "ps"] and "container" not in seen:
                    seen.add("container")
                    return b"abcdef123456\n"
                if args[:3] == ["docker", "volume", "ls"] and "volume" not in seen:
                    seen.add("volume")
                    return (args[-1].split("=", 2)[-1] + "\n").encode()
                return run(args, **kwargs)
            with self.assertRaises(Refusal):
                qualify(archive, fixture_identity(component="runtime"), "sha256:" + "d" * 64,
                        "e" * 64, root, abandoned)
            self.assertTrue(any(args == ["docker", "rm", "--force", "abcdef123456"] for args, _ in calls))
            self.assertTrue(any(args[:3] == ["docker", "volume", "rm"] for args, _ in calls))
            self.assertEqual(calls[-1][0][:3], ["docker", "image", "rm"])

    def test_nonruntime_uses_no_smoke_operations(self):
        run = Mock()
        self.assertIsNone(qualify(None, fixture_identity(), None, None, None, run))
        run.assert_not_called()

    def test_failed_canary_never_authenticates_or_publishes(self):
        with tempfile.TemporaryDirectory() as directory:
            archive, digest, registry, scanner, run = PublicationTests().fixture(directory)
            with patch("staging_cd_publish.inspect", return_value=digest), self.assertRaises(Refusal):
                publish(archive, fixture_identity(component="runtime"), Path(directory), registry,
                        "fixture", "actor", scanner=scanner, runner=run,
                        smoke=Mock(side_effect=Refusal("canary_failed")))
            run.assert_not_called()

    def test_successful_publication_preserves_canary_and_scan_bindings(self):
        with tempfile.TemporaryDirectory() as directory:
            archive, digest, registry, scanner, run = PublicationTests().fixture(directory)
            def smoke(_archive, identity, image_digest, scanned, *_args, **_kwargs):
                return dict(fixture_runtime_receipt()["runtime_smoke"], digest=image_digest,
                            archive=scanned, source=identity["source"])
            with patch("staging_cd_publish.inspect", return_value=digest):
                receipt = publish(archive, fixture_identity(component="runtime"), Path(directory), registry,
                                  "fixture", "actor", scanner=scanner, runner=run, smoke=smoke)
            self.assertEqual(receipt["runtime_smoke"]["archive"], receipt["archive"])
            self.assertEqual(release_set({"runtime": receipt})["receipts"]["runtime"], receipt)

    def test_malformed_or_old_unqualified_receipts_cannot_promote(self):
        receipt = fixture_runtime_receipt()
        for field, value in (("digest", "sha256:" + "a" * 64), ("source", "b" * 40),
                             ("archive", "0" * 64), ("passed", 1), ("evidence", "Z" * 64)):
            bad = copy.deepcopy(receipt)
            bad["runtime_smoke"][field] = value
            with self.assertRaises(Refusal):
                validate_receipt(bad)
        old = copy.deepcopy(receipt)
        old.pop("runtime_smoke")
        validate_receipt(old)  # Historical publication remains readable.
        with self.assertRaises(Refusal):
            release_set({"runtime": old})
        with self.assertRaises(Refusal):
            validate_smoke(None, source=receipt["source"], digest=receipt["digest"], archive=receipt["archive"])
