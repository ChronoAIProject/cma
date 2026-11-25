"""Keep Web continuity evidence exact through publication and private admission."""

import copy
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

from cd_target import PRODUCTION
from staging_cd_common import Refusal
from staging_cd_deploy_result import SCHEMA as RESULT_SCHEMA, emit, report
from staging_cd_history import DevelopHistory
from staging_cd_promotion import proven_components
from staging_cd_receipts import annotation, canonical, parse_annotation, published_receipt, sha256, validate_receipt
from staging_cd_receipts_tests import fixture_receipt, fixture_tag
from staging_cd_resolve import verify
from staging_cd_set import private_document, release_set, verify_set


REPOSITORY = "ghcr.io/chronoaiproject/cma-web"


def image(digit):
    return REPOSITORY + "@sha256:" + digit * 64


def fixture_continuity():
    return {
        "schema": "cma-web-asset-continuity-v1",
        "incumbent": {"image": image("1"), "base_image": image("2"),
                      "source_sha": "b" * 40, "version": "0.10.0"},
        "candidate_base_image": image("3"),
        "prepare_image": image("4"),
        "asset_manifest_sha256": "5" * 64,
        "proof_sha256": "6" * 64,
        "sentinels": {
            "incumbent": {"path": "/assets/index-before.js", "sha256": "7" * 64, "html_sha256": "8" * 64},
            "candidate": {"path": "/assets/index-after.js", "sha256": "9" * 64, "html_sha256": "a" * 64},
        },
    }


class WebContinuityTests(unittest.TestCase):
    def setUp(self):
        self.original = fixture_receipt()
        self.continuity = fixture_continuity()
        self.receipt = self.publish(self.original, asset_continuity=self.continuity)
        self.history = DevelopHistory("a" * 40, ["a" * 40, "b" * 40])
        self.registry = Mock()
        self.registry.exists.return_value = True
        self.registry.manifest.return_value = (self.receipt["digest"], {})

    def publish(self, original, **options):
        scan = original["scan"]
        return published_receipt(original["identity"], original["digest"], original["archive"],
                                 scan["counts"], scanner_identity=(scan["version"], scan["bundle"]), **options)

    def test_public_release_keeps_exact_final_overlay_digest(self):
        self.assertEqual(self.receipt["schema"], "cma-published-image-v3")
        self.assertEqual(self.receipt["digest"], self.original["digest"])
        self.assertEqual(self.receipt["asset_continuity"], self.continuity)
        self.assertEqual(parse_annotation(annotation(self.receipt)), self.receipt)
        self.assertEqual(self.publish(self.original), self.original)

    def test_staging_projects_protected_web_only_and_production_keeps_format(self):
        receipts = {"web": self.receipt}
        release = release_set(receipts)
        staged = private_document(release)
        self.assertEqual(staged["schema"], 3)
        exact = private_document(release_set(dict(receipts, web=self.original)))
        old_web = exact["components"]["web"]
        self.assertEqual(staged["components"]["web"], dict(old_web, asset_continuity=self.continuity))
        self.assertEqual(private_document(release, target=PRODUCTION), exact)
        self.assertEqual(private_document(release_set({"web": self.original}))["schema"], 1)

    def test_mixed_staging_set_cannot_interleave_with_two_phase_web(self):
        am = fixture_receipt(component="am")
        mixed = release_set({"web": self.receipt, "am": am})
        with self.assertRaisesRegex(Refusal, "standalone staging release"):
            private_document(mixed)
        legacy = private_document(release_set({"web": self.original, "am": am}))
        self.assertEqual(legacy["schema"], 2)
        self.assertEqual(private_document(mixed, target=PRODUCTION), legacy)

    def test_filtering_an_admitted_release_preserves_exact_evidence_only_with_web(self):
        value = release_set({"web": self.receipt})
        api = Mock()
        api.tags.return_value = [fixture_tag(self.receipt)]
        api.history.return_value = self.history
        self.assertEqual(verify_set(value, api, self.registry, automatic=True), value)
        stage = private_document(verify_set(value, api, self.registry, automatic=True))
        self.assertEqual(stage["components"]["web"]["asset_continuity"], self.continuity)
        without_web = private_document(release_set({"eg": fixture_receipt(component="eg")}))
        self.assertEqual(without_web["schema"], 1)
        self.assertNotIn("asset_continuity", without_web["components"]["eg"])

    def test_controller_result_binds_the_exact_target_and_promote_image(self):
        selected = release_set({"web": self.receipt})
        request = private_document(selected)
        web = request["components"]["web"]
        result = {"schema": RESULT_SCHEMA, "request": sha256(request), "code": 0, "components": {
            "web": {"image": web["image"], "source_sha": web["source_sha"],
                    "state": "controller-verified", "reason": "none"},
        }}
        env = {"CD_RELEASE_SET": canonical(selected), "CD_COMPONENT_RESULT": canonical(result)}
        with patch("staging_cd_deploy_result.retain", side_effect=lambda value, _: value):
            self.assertEqual(report(env), result)
            with self.assertRaises(Refusal):
                report(dict(env, CD_TARGET=PRODUCTION))
            result["components"]["web"]["image"] = self.continuity["candidate_base_image"]
            with self.assertRaises(Refusal):
                report(dict(env, CD_COMPONENT_RESULT=canonical(result)))

    def test_promotion_skip_proof_binds_the_protected_web_fields(self):
        request = private_document(release_set({"web": self.receipt}))
        component = request["components"]["web"]
        result = {"schema": RESULT_SCHEMA, "request": sha256(request), "code": 0, "components": {
            "web": {"image": component["image"], "source_sha": component["source_sha"],
                    "state": "controller-verified", "reason": "none"},
        }}
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "github-output"
            emit(result, request, 0, {"GITHUB_OUTPUT": str(output)})
            stamped = dict(line.split("=", 1) for line in output.read_text().splitlines())
        evidence = {"web": {"writer": self.receipt["source"], "stamp": stamped["controller_proof"]}}
        self.assertEqual(proven_components(evidence, {"web": self.receipt}, self.history), {"web"})
        changed = dict(self.continuity, proof_sha256="e" * 64)
        receipt = self.publish(self.original, asset_continuity=changed)
        self.assertEqual(proven_components(evidence, {"web": receipt}, self.history), set())

    def test_registry_must_still_have_all_release_images(self):
        tags = [fixture_tag(self.receipt)]
        self.assertEqual(verify(self.receipt, tags, self.registry, self.history, automatic=True), self.receipt)
        for digit in "1234":
            with self.subTest(digest=digit):
                missing = "sha256:" + digit * 64
                self.registry.exists.side_effect = lambda repository, value: value != missing
                with self.assertRaisesRegex(Refusal, "Web continuity image is not publicly available"):
                    verify(self.receipt, tags, self.registry, self.history, automatic=True)

    def test_web_only_receipt_and_schema_are_explicit(self):
        for component in ("am", "eg"):
            with self.subTest(component=component), self.assertRaises(Refusal):
                self.publish(fixture_receipt(component=component), asset_continuity=self.continuity)
        for corrupt in (dict(self.receipt, schema="cma-published-image-v1"),
                        dict(self.original, schema="cma-published-image-v3"),
                        dict(self.receipt, compatibility={})):
            with self.assertRaises(Refusal):
                validate_receipt(corrupt)

    def test_changed_or_incomplete_protected_fields_are_refused(self):
        mutations = (
            lambda row: row.pop("proof_sha256"),
            lambda row: row.update(asset_manifest_sha256="sha256:" + "5" * 64),
            lambda row: row.update(proof_sha256="UPPERCASE"),
            lambda row: row.update(prepare_image=REPOSITORY + ":stable"),
            lambda row: row.update(prepare_image=image("0")),
            lambda row: row.update(prepare_image=image("d")),
            lambda row: row.update(candidate_base_image=image("4")),
            lambda row: row["incumbent"].update(source_sha="a" * 40),
            lambda row: row["sentinels"].pop("candidate"),
            lambda row: row["sentinels"]["candidate"].update(path="/assets/../build-info.json"),
            lambda row: row["sentinels"]["candidate"].update(path="/assets/index-before.js"),
            lambda row: row["sentinels"]["candidate"].update(html_sha256=None),
        )
        for index, mutate in enumerate(mutations):
            with self.subTest(mutation=index):
                continuity = copy.deepcopy(self.continuity)
                mutate(continuity)
                with self.assertRaises(Refusal):
                    self.publish(self.original, asset_continuity=continuity)
