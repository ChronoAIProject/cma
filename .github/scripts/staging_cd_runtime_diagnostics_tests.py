"""Publication diagnostics preserve runtime evidence and refusal boundaries."""
import contextlib
import io
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

from image_secret_gate import checksum, ScanRefusal
from staging_cd_build import command
from staging_cd_common import Refusal
from staging_cd_contract_tests import fixture_source
from staging_cd_publication_diagnostics import ImageCommandRefusal, run_publication
from staging_cd_publish import publish
import staging_cd_publish_tests as publication_fixtures
from staging_cd_receipts_tests import fixture_identity, fixture_runtime_receipt
from staging_cd_runtime_smoke import CleanupIncomplete
from staging_cd_set import private_document, release_set
from staging_cd_outcome_tests import legacy


class RuntimeDiagnosticsTests(unittest.TestCase):
    def observe(self, operation):
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            status = run_publication(operation)
        return status, legacy(output.getvalue())

    def test_runtime_publication_preserves_smoke_and_bootstrap_after_diagnostics(self):
        for component in ("runtime", "runtime-omarchy"):
            with self.subTest(component=component), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                archive, digest, registry, scanner, runner = publication_fixtures.PublicationTests().fixture(directory)
                identity = fixture_identity(component=component)
                proof = dict(fixture_runtime_receipt(component)["runtime_smoke"], archive=checksum(archive))
                def qualify(*args, **kwargs):
                    self.assertEqual(args[:4], (archive, identity, digest, checksum(archive)))
                    self.assertEqual(kwargs["source"], root / "source")
                    runner.assert_not_called()
                    return proof
                receipts = []
                def operation(_masker, diagnostics):
                    receipt = publish(archive, identity, root, registry, "fixture", "actor",
                                      reservation_check=lambda: True, scanner=scanner, runner=runner, smoke=qualify,
                                      source=fixture_source(root / "source"), diagnostics=diagnostics)
                    receipts.append(receipt)
                    self.assertEqual(diagnostics.stage, "receipt")
                    return True
                with patch("staging_cd_publish.inspect", return_value=digest):
                    self.assertEqual(self.observe(operation), (0, "PASS publication\n"))
                receipt = receipts[0]
                self.assertEqual(receipt["runtime_smoke"], proof)
                encoded = private_document(release_set({component: receipt}))["components"][component]
                self.assertEqual(encoded["runtime_smoke"], proof)
                self.assertEqual(encoded["runtime_bootstrap"]["schema"], "cma-runtime-bootstrap-request-v1")
                self.assertEqual([call.args[0][:2] for call in runner.call_args_list],
                                 [["skopeo", "login"], ["skopeo", "copy"]])
                self.assertFalse((root / "registry-auth.json").exists())

    def test_runtime_refusals_emit_only_safe_stages_before_any_registry_writer(self):
        cases = {"scan": ("scan", "scan_refused"), "smoke": ("runtime_smoke", "validation_refused"),
                 "cleanup": ("runtime_smoke", "runtime_cleanup_incomplete"),
                 "changed": ("archive_check", "validation_refused"),
                 "missing": ("archive_check", "validation_refused"),
                 "source": ("receipt", "validation_refused")}
        for component in ("runtime", "runtime-omarchy"):
            for failure, (stage, reason) in cases.items():
                with self.subTest(component=component, failure=failure), tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    archive, digest, registry, scanner, runner = publication_fixtures.PublicationTests().fixture(directory)
                    def qualify(_archive, identity, _digest, scanned, *_args, **_kwargs):
                        if failure == "smoke":
                            raise Refusal("private helper output\n::warning::injected")
                        if failure == "cleanup":
                            raise CleanupIncomplete("cma-runtime-ci-" + "a" * 36, False, False)
                        if failure == "missing":
                            return None
                        if failure == "changed":
                            archive.write_bytes(b"changed image after smoke")
                        source = "b" * 40 if failure == "source" else identity["source"]
                        return dict(fixture_runtime_receipt(component)["runtime_smoke"], archive=scanned, source=source)
                    smoke = Mock(side_effect=qualify)
                    if failure == "scan":
                        scanner = Mock(side_effect=ScanRefusal("private scan output"))
                    def operation(_masker, diagnostics):
                        return publish(archive, fixture_identity(component=component), root, registry,
                                       "fixture", "actor", reservation_check=lambda: True, scanner=scanner, runner=runner,
                                       smoke=smoke, source=fixture_source(root / "source"), diagnostics=diagnostics)
                    with patch("staging_cd_publish.inspect", return_value=digest):
                        self.assertEqual(self.observe(operation),
                                         (1, f"FAIL publication\nFAIL publication stage={stage} reason={reason}\n"))
                    runner.assert_not_called()
                    registry.exists.assert_not_called()
                    self.assertFalse((root / "registry-auth.json").exists())
                    if failure == "scan":
                        smoke.assert_not_called()

    def test_registry_denial_after_runtime_qualification_remains_sanitized(self):
        for component in ("runtime", "runtime-omarchy"):
            with self.subTest(component=component), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                archive, digest, registry, scanner, runner = publication_fixtures.PublicationTests().fixture(directory)
                login = runner.side_effect
                def denied(args, **kwargs):
                    if args[:2] == ["skopeo", "copy"]:
                        raise ImageCommandRefusal(args, b"DENIED private registry response")
                    return login(args, **kwargs)
                runner.side_effect = denied
                proof = dict(fixture_runtime_receipt(component)["runtime_smoke"], archive=checksum(archive))
                smoke = Mock(return_value=proof)
                def operation(_masker, diagnostics):
                    return publish(archive, fixture_identity(component=component), root, registry,
                                   "fixture", "actor", reservation_check=lambda: True, scanner=scanner, runner=runner,
                                   smoke=smoke, source=fixture_source(root / "source"), diagnostics=diagnostics)
                with patch("staging_cd_publish.inspect", return_value=digest):
                    self.assertEqual(self.observe(operation),
                                     (1, "FAIL publication\nFAIL publication stage=registry_copy reason=registry_denied\n"))
                smoke.assert_called_once()
                self.assertEqual(runner.call_count, 2)
                registry.exists.assert_not_called()
                self.assertFalse((root / "registry-auth.json").exists())

    def test_canary_input_credentials_are_rejected_before_either_runner(self):
        with patch("staging_cd_build.canary_run") as canary, patch("staging_cd_build.subprocess.run") as ordinary:
            with self.assertRaisesRegex(Refusal, "Runtime canary cannot receive input credentials"):
                command(["node", "fixture"], data=b"fixture", env={"CMA_RUNTIME_EVIDENCE_ID": "fixture"})
            canary.assert_not_called()
            ordinary.assert_not_called()


if __name__ == "__main__":
    unittest.main()
