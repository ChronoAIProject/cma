"""Exact staging image qualification and bounded fallback on the CI lane."""

import json
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

import ci_retry
from ci_staging_image import adopt, await_ref, matching_receipt, qualification, refs, validate_loaded, WAIT_SECONDS
from staging_cd_common import Refusal
from staging_cd_contract import labels, RECIPE_LABELS
from staging_cd_history import DevelopHistory
from staging_cd_receipts import IDENTITY_LABEL, annotation, canonical, final_tag_oid
from staging_cd_receipts_tests import fixture_receipt, fixture_runtime_receipt
from staging_cd_registry import MEDIA


def final(receipt):
    oid = final_tag_oid(receipt)
    release = receipt["identity"]
    tag = {"sha": oid, "tag": receipt["tag"], "message": annotation(receipt),
           "object": {"type": "commit", "sha": receipt["source"], "url": "https://example.invalid"}}
    run = {"id": release["run_id"], "run_attempt": release["run_attempt"], "event": "push",
           "head_branch": "develop", "head_sha": receipt["source"], "path": ".github/workflows/staging-cd.yml",
           "repository": {"full_name": "ChronoAIProject/cma"}, "head_repository": {"full_name": "ChronoAIProject/cma"}}
    return "refs/tags/" + receipt["tag"], oid, tag, run


class StagingImageTests(unittest.TestCase):
    def test_resolve_only_a_peeled_final_of_this_source(self):
        receipt = fixture_receipt(component="am")
        ref, oid, _, _ = final(receipt)
        first = f"{oid}\t{ref}\n"
        other = f"{'9' * 40}\t{ref}^{{}}\n"
        correct = f"{receipt['source']}\t{ref}^{{}}\n"
        source = receipt["source"]
        self.assertIsNone(refs("am", source, Mock(return_value=first)))
        self.assertIsNone(refs("am", source, Mock(return_value=first + other)))
        self.assertEqual(refs("am", source, Mock(return_value=first + correct)), (ref, oid))
        with self.assertRaisesRegex(Refusal, "annotated object"):
            refs("am", source, Mock(return_value=correct))

    def test_reject_direct_final_even_alongside_an_annotated_final(self):
        receipt = fixture_receipt(component="am")
        ref, oid, _, _ = final(receipt)
        source = receipt["source"]
        direct = f"{source}\trefs/tags/cma-agent-manager/v0.0.99\n"
        annotated = f"{oid}\t{ref}\n{source}\t{ref}^{{}}\n"
        for inventory in (direct, annotated + direct):
            with self.subTest(inventory=inventory), self.assertRaisesRegex(Refusal, "must be an annotated tag"):
                refs("am", source, Mock(return_value=inventory))

    def test_final_binds_exact_source_tree_and_workflow_attempt(self):
        receipt = fixture_runtime_receipt()
        ref, oid, tag, run = final(receipt)
        build = receipt["identity"]
        for changes in ({}, {"event": "pull_request"}, {"run_attempt": 99}, {"path": ".github/workflows/ci.yml"}):
            with self.subTest(changes=changes):
                api = Mock()
                api.request.side_effect = [tag, dict(run, **changes)]
                if changes:
                    with self.assertRaisesRegex(Refusal, "different workflow"):
                        matching_receipt(ref, oid, "runtime", receipt["source"], build["tree"], api)
                else:
                    self.assertEqual(matching_receipt(ref, oid, "runtime", receipt["source"], build["tree"], api),
                                     receipt)
        api = Mock()
        api.request.return_value = tag
        with self.assertRaisesRegex(Refusal, "differs from CI source"):
            matching_receipt(ref, oid, "runtime", receipt["source"], "e" * 40, api)

    def test_backlog_final_from_a_later_develop_push_uses_fallback(self):
        receipt = fixture_runtime_receipt()
        ref, oid, tag, run = final(receipt)
        source = receipt["source"]
        tree = receipt["identity"]["tree"]
        later = "9" * 40
        api = Mock()
        api.request.side_effect = [tag, dict(run, head_sha=later)]
        api.history_containing.return_value = DevelopHistory(later, [later, source])
        self.assertIsNone(matching_receipt(ref, oid, "runtime", source, tree, api))
        api.history_containing.assert_called_once_with(later)

        for bad_head, history in (("0" * 40, DevelopHistory(later, [later, source])),
                                  (later, DevelopHistory(source, [source, later]))):
            with self.subTest(head=bad_head):
                api.request.side_effect = [tag, dict(run, head_sha=bad_head)]
                api.history_containing.return_value = history
                with self.assertRaisesRegex(Refusal, "first-parent history|does not precede"):
                    matching_receipt(ref, oid, "runtime", source, tree, api)

    def test_transient_history_read_is_unreachable(self):
        receipt = fixture_runtime_receipt()
        ref, oid, tag, run = final(receipt)
        source = receipt["source"]
        api = Mock()
        api.request.side_effect = [tag, dict(run, head_sha="9" * 40)]
        outage = Refusal("Develop ancestry could not be verified")
        outage.__cause__ = ci_retry.TransientError()
        api.history_containing.side_effect = outage
        env = {"GITHUB_EVENT_NAME": "push", "GITHUB_SHA": source, "IMAGE_COMPONENT": "runtime"}
        with patch("ci_staging_image.event_identity"), patch("ci_staging_image.GitHub", return_value=api), \
                patch("ci_staging_image.await_ref", return_value=((ref, oid), None)), \
                patch("ci_staging_image.checked_command", side_effect=[source, receipt["identity"]["tree"]]):
            self.assertEqual(qualification(env, Path.cwd()), (None, "unreachable"))

    def test_wait_ends_at_the_deadline_and_returns_a_later_final(self):
        time = [7.0]
        clock = lambda: time[0]
        sleep = lambda delay: time.__setitem__(0, time[0] + delay)
        lookup = Mock(side_effect=[None, ("fixture-ref", "fixture-tag")])
        self.assertEqual(await_ref("am", "source", lookup=lookup, clock=clock, sleep=sleep),
                         (("fixture-ref", "fixture-tag"), None))
        time[0] = 7.0
        self.assertEqual(await_ref("am", "source", lookup=Mock(return_value=None), clock=clock, sleep=sleep),
                         (None, "no-final"))
        self.assertEqual(time[0], 7.0 + WAIT_SECONDS)

    def test_adoption_checks_manifest_config_and_tag_before_reporting_the_digest(self):
        receipt = fixture_runtime_receipt(component="runtime-omarchy")
        release = receipt["identity"]
        compatibility = receipt["compatibility"]
        digest = receipt["digest"]
        reference = receipt["repository"] + "@" + digest
        config = "sha256:" + "c" * 64
        target = "cma-runtime-omarchy:ci"
        row = {
            "RepoDigests": [reference], "Id": config, "Architecture": "amd64", "Os": "linux",
            "Config": {"Labels": {**labels(compatibility), IDENTITY_LABEL: canonical(release),
                                 "org.opencontainers.image.revision": release["source"],
                                 "org.opencontainers.image.version": release["version"],
                                 **{label: compatibility[key] for key, label in RECIPE_LABELS.items()}}},
        }
        registry = Mock()
        registry.manifest.return_value = digest, {"config": {"digest": config}}
        command = Mock(side_effect=["", json.dumps([row]), "", json.dumps([row])])
        with patch("ci_staging_image.identity", return_value=(release, {}, {})), \
                patch("ci_staging_image.from_source", return_value=compatibility):
            self.assertEqual(adopt(receipt, target, Path.cwd(), registry, run=command), digest)
        self.assertEqual(command.call_args_list[0].args[0], ["docker", "pull", "--platform", "linux/amd64", reference])
        self.assertEqual(command.call_args_list[2].args[0], ["docker", "tag", reference, target])
        with self.assertRaisesRegex(Refusal, "config differs"):
            validate_loaded(dict(row, Id=digest), reference, config, release, compatibility)
        descriptor = {"digest": digest, "mediaType": MEDIA.split(", ")[0]}
        validate_loaded(dict(row, Id=digest, Descriptor=descriptor), reference, config, release, compatibility)
        with self.assertRaisesRegex(Refusal, "descriptor differs"):
            validate_loaded(dict(row, Id=digest, Descriptor=dict(descriptor, digest=config)), reference, config,
                            release, compatibility)


if __name__ == "__main__":
    unittest.main()
