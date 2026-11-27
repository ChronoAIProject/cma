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
from staging_cd_promotion import proof as promotion_proof, proven_components
from staging_cd_receipts import annotation, canonical, parse_annotation, published_receipt, sha256, validate_receipt
from staging_cd_receipts_tests import fixture_receipt, fixture_tag
from staging_cd_resolve import verify
from staging_cd_set import private_document, release_set, verify_set
from staging_cd_web_continuity import (
    BOOTSTRAP_EVIDENCE_SCHEMA, BOOTSTRAP_SCHEMA, PREIMAGE_SCHEMA, PROOF_SCHEMA, binding, retention_scope,
)


REPOSITORY = "ghcr.io/chronoaiproject/cma-web"
STARTED = "2026-01-01T00:00:00Z"


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


def fixture_proof(continuity, release):
    def overlay(image, base, archive):
        return {"image": image, "base_image": base, "layer_digest": "sha256:" + "b" * 64,
                "archive_sha256": archive, "credential_report_sha256": "e" * 64}

    return {
        "schema": PROOF_SCHEMA, "continuity_sha256": binding(continuity),
        "retention_preimage_sha256": "d" * 64, "asset_count": 201, "asset_bytes": 10001,
        "inputs": [
            {"image": image("2"), "source_sha": "b" * 40, "version": "0.10.0", "catalog_sha256": "2" * 64,
             "completed_rollouts": 0},
            {"image": image("3"), "source_sha": release["source"], "version": release["identity"]["version"],
             "catalog_sha256": "3" * 64, "completed_rollouts": 0},
            {"image": image("5"), "source_sha": "c" * 40, "version": "0.9.0", "catalog_sha256": "1" * 64,
             "completed_rollouts": 2},
        ],
        "overlays": {"prepare": overlay(image("4"), image("2"), "c" * 64),
                     "promote": overlay(REPOSITORY + "@" + release["digest"], image("3"), release["archive"])},
    }


def fixture_preimage(incumbent, manifest=None):
    return {"schema": PREIMAGE_SCHEMA, "observed_at": STARTED, "incumbent": incumbent, "asset_manifest_sha256": manifest}


class WebContinuityTests(unittest.TestCase):
    def setUp(self):
        self.original = fixture_receipt()
        self.continuity = fixture_continuity()
        self.proof = fixture_proof(self.continuity, self.original)
        self.continuity["proof_sha256"] = sha256(self.proof)
        self.receipt = self.publish(self.original, asset_continuity=self.continuity, asset_proof=self.proof)
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
        self.assertEqual(self.receipt["asset_proof"], self.proof)
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
        changed_proof = dict(self.proof, retention_preimage_sha256="e" * 64)
        changed = dict(self.continuity, proof_sha256=sha256(changed_proof))
        receipt = self.publish(self.original, asset_continuity=changed, asset_proof=changed_proof)
        self.assertEqual(proven_components(evidence, {"web": receipt}, self.history), set())

    def test_legacy_mixed_promotion_stamp_cannot_stop_protected_web_allocation(self):
        eg = fixture_receipt(component="eg")
        legacy = {"web": self.original, "eg": eg}
        stamp = promotion_proof(private_document(release_set(legacy))["components"])
        evidence = {component: {"writer": self.receipt["source"], "stamp": stamp} for component in legacy}
        self.assertEqual(proven_components(evidence, legacy, self.history), {"web", "eg"})
        self.assertEqual(proven_components(evidence, dict(legacy, web=self.receipt), self.history), set())

    def test_registry_must_still_have_all_release_images(self):
        tags = [fixture_tag(self.receipt)]
        self.assertEqual(verify(self.receipt, tags, self.registry, self.history, automatic=True), self.receipt)
        for digit in "12345":
            with self.subTest(digest=digit):
                missing = "sha256:" + digit * 64
                self.registry.exists.side_effect = lambda repository, value: value != missing
                with self.assertRaisesRegex(Refusal, "Web continuity image is not publicly available"):
                    verify(self.receipt, tags, self.registry, self.history, automatic=True)

    def test_web_only_receipt_and_schema_are_explicit(self):
        for component in ("am", "eg"):
            with self.subTest(component=component), self.assertRaises(Refusal):
                self.publish(fixture_receipt(component=component), asset_continuity=self.continuity,
                             asset_proof=self.proof)
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
                    self.publish(self.original, asset_continuity=continuity, asset_proof=self.proof)

    def test_detached_proof_is_required_and_mutating_only_its_hash_refuses(self):
        with self.assertRaises(Refusal):
            self.publish(self.original, asset_continuity=self.continuity)
        with self.assertRaisesRegex(Refusal, "publication proof hash differs"):
            self.publish(self.original, asset_continuity=dict(self.continuity, proof_sha256="f" * 64),
                         asset_proof=self.proof)

    def test_new_hash_cannot_override_the_exact_verified_release_shape(self):
        mutations = (
            lambda proof: proof.update(continuity_sha256="f" * 64),
            lambda proof: proof["inputs"].reverse(),
            lambda proof: proof["inputs"].pop(0),
            lambda proof: proof["inputs"][1].update(completed_rollouts=1),
            lambda proof: proof["inputs"][2].update(completed_rollouts=True),
            lambda proof: proof["overlays"]["prepare"].update(base_image=image("3")),
            lambda proof: proof["overlays"]["prepare"].update(layer_digest="sha256:" + "a" * 64),
            lambda proof: proof["overlays"]["promote"].update(archive_sha256="f" * 64),
        )
        for mutate in mutations:
            with self.subTest(mutation=mutate):
                proof = copy.deepcopy(self.proof)
                mutate(proof)
                continuity = dict(self.continuity, proof_sha256=sha256(proof))
                with self.assertRaises(Refusal):
                    self.publish(self.original, asset_continuity=continuity, asset_proof=proof)

    def test_next_successor_keeps_earlier_exposure_across_promote_or_rollback(self):
        promoted = {"image": REPOSITORY + "@" + self.receipt["digest"],
                    "base_image": self.continuity["candidate_base_image"], "source_sha": self.receipt["source"],
                    "version": self.receipt["identity"]["version"]}
        after_rollback = dict(self.continuity["incumbent"], image=self.continuity["prepare_image"])
        manifest = self.continuity["asset_manifest_sha256"]
        prepared_preimage = fixture_preimage(after_rollback, manifest)
        current = retention_scope(prepared_preimage, STARTED, [self.receipt], retain_rollouts=1)
        self.assertEqual(current, {"incumbent": after_rollback, "preimage_sha256": sha256(prepared_preimage),
                                   "inputs": self.proof["inputs"]})
        promoted_preimage = fixture_preimage(promoted, manifest)
        wider = retention_scope(promoted_preimage, STARTED, [self.receipt])
        self.assertEqual([row["completed_rollouts"] for row in wider["inputs"]], [1, 0, 3])
        narrower = retention_scope(promoted_preimage, STARTED, [self.receipt], retain_rollouts=2)
        self.assertEqual([row["image"] for row in narrower["inputs"]], [image("2"), image("3")])
        self.assertEqual([row["completed_rollouts"] for row in narrower["inputs"]], [1, 0])
        with self.assertRaisesRegex(Refusal, "differs from its finalized lineage"):
            incorrect = dict(after_rollback, base_image=self.continuity["candidate_base_image"])
            retention_scope(fixture_preimage(incorrect, manifest), STARTED, [self.receipt])
        with self.assertRaisesRegex(Refusal, "manifest differs"):
            retention_scope(fixture_preimage(after_rollback, "b" * 64), STARTED, [self.receipt])
        with self.assertRaisesRegex(Refusal, "differs from its admission"):
            retention_scope(fixture_preimage(after_rollback, manifest), "2026-01-01T00:01:00Z", [self.receipt])

    def test_default_retention_prunes_only_after_the_sixth_successor(self):
        proof = copy.deepcopy(self.proof)
        proof["inputs"][2]["completed_rollouts"] = 6
        proof["inputs"].append({
            "image": image("7"), "source_sha": "d" * 40, "version": "0.9.1",
            "catalog_sha256": "7" * 64, "completed_rollouts": 5,
        })
        continuity = dict(self.continuity, proof_sha256=sha256(proof))
        receipt = self.publish(self.original, asset_continuity=continuity, asset_proof=proof)
        manifest = continuity["asset_manifest_sha256"]

        prepared = dict(continuity["incumbent"], image=continuity["prepare_image"])
        rollback_scope = retention_scope(fixture_preimage(prepared, manifest), STARTED, [receipt])
        self.assertEqual(rollback_scope["inputs"], proof["inputs"])

        promoted = {
            "image": REPOSITORY + "@" + receipt["digest"],
            "base_image": continuity["candidate_base_image"],
            "source_sha": receipt["source"],
            "version": receipt["identity"]["version"],
        }
        settled_scope = retention_scope(fixture_preimage(promoted, manifest), STARTED, [receipt])
        self.assertEqual(
            [(row["image"], row["completed_rollouts"]) for row in settled_scope["inputs"]],
            [(image("2"), 1), (image("3"), 0), (image("7"), 6)],
        )

    def test_legacy_raw_preimage_requires_bootstrap_and_full_identity(self):
        legacy = {"image": REPOSITORY + "@" + self.original["digest"],
                  "base_image": REPOSITORY + "@" + self.original["digest"], "source_sha": self.original["source"],
                  "version": self.original["identity"]["version"]}
        preimage = fixture_preimage(legacy)
        earlier = dict(fixture_receipt(source="c" * 40, previous="0.9.0", version="0.9.1"), digest="sha256:" + "5" * 64)
        before = {"image": REPOSITORY + "@" + earlier["digest"], "source_sha": earlier["source"],
                  "version": earlier["identity"]["version"]}
        deployed = {"run_id": 4100, "run_attempt": 1, "publication_receipt_sha256": "b" * 64,
                    "controller_result_sha256": "c" * 64}
        current = {key: legacy[key] for key in ("image", "source_sha", "version")}
        evidence = {"schema": BOOTSTRAP_EVIDENCE_SCHEMA, "deployments": [
            dict(deployed, **before), dict(deployed, **current),
        ]}
        bootstrap = {"schema": BOOTSTRAP_SCHEMA, "incumbent": legacy, "approved_deployments_sha256": sha256(evidence),
                     "retained": [before]}
        options = {"bootstrap": bootstrap, "bootstrap_evidence": evidence}
        self.assertEqual(retention_scope(preimage, STARTED, [self.original, earlier], **options),
                         {"incumbent": legacy, "inputs": [dict(before, completed_rollouts=0)],
                          "preimage_sha256": sha256({"controller": preimage, "bootstrap": bootstrap})})
        with self.assertRaisesRegex(Refusal, "requires an approved bootstrap lineage"):
            retention_scope(preimage, STARTED, [self.original])
        with self.assertRaisesRegex(Refusal, "lineage differs from finalized"):
            retention_scope(preimage, STARTED, [self.original], **options)
        with self.assertRaisesRegex(Refusal, "no unique finalized lineage"):
            retention_scope(preimage, STARTED, [self.original, self.original], **options)
        mutated = dict(bootstrap, approved_deployments_sha256="0" * 64)
        with self.assertRaisesRegex(Refusal, "evidence differs from its approval"):
            retention_scope(preimage, STARTED, [self.original, earlier],
                            bootstrap=mutated, bootstrap_evidence=evidence)
        incomplete = dict(evidence, deployments=[dict(deployed, **current)])
        with self.assertRaisesRegex(Refusal, "does not cover the retained and incumbent images"):
            retention_scope(preimage, STARTED, [self.original, earlier], bootstrap_evidence=incomplete,
                            bootstrap=dict(bootstrap, approved_deployments_sha256=sha256(incomplete)))
