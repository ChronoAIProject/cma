"""Manual and automatic redeployment preserve immutable release identity."""
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

from staging_cd_admission import event_identity, maintainer_dispatch
from staging_cd_common import Refusal
from staging_cd_history import DevelopHistory
from staging_cd_receipts_tests import fixture_receipt, fixture_tag
from staging_cd_resolve import components, main, resolve, verify


class ResolveTests(unittest.TestCase):
    def setUp(self):
        self.history = DevelopHistory("f" * 40, ["f" * 40, "a" * 40])

    def test_expired_automatic_web_stamp_is_replaced_only_after_verifying_candidate(self):
        release = {"receipts": {"web": fixture_receipt()}}
        env = {"CD_RESOLVE_MODE": "verify-web-at-deploy", "CD_DEPLOY_MODE": "release",
               "CD_STARTED_AT": "2000-01-01T00:00:00Z", "GITHUB_EVENT_NAME": "push",
               "CD_RELEASE_SET": json.dumps(release)}
        trace = []
        def verify_candidate(candidate, *_args, **kwargs):
            self.assertEqual(candidate, release)
            self.assertTrue(kwargs["automatic"])
            trace.append("verified")
            return candidate
        def stamp():
            self.assertEqual(trace, ["verified"])
            return "2026-11-28T00:00:01Z"
        with tempfile.TemporaryDirectory() as temp, \
                patch.dict(os.environ, env | {"GITHUB_OUTPUT": str(Path(temp) / "out")}, clear=True), \
                patch("staging_cd_resolve.GitHub"), patch("staging_cd_resolve.Registry"), \
                patch("staging_cd_resolve.event_identity"), patch("staging_cd_clock.run_created") as authorize, \
                patch("staging_cd_set.verify_set", side_effect=verify_candidate) as verify_set, \
                patch("staging_cd_clock.timestamp", side_effect=stamp) as timestamp, \
                patch("staging_cd_clock.remaining") as remaining:
            self.assertEqual(main(), 0)
            authorize.assert_called_once()
            remaining.assert_called_once_with("2026-11-28T00:00:01Z", mode="release", env=os.environ)
            self.assertIn("started_at=2026-11-28T00:00:01Z", (Path(temp) / "out").read_text())
            verify_set.reset_mock(side_effect=True)
            timestamp.reset_mock()
            for change in ({"GITHUB_EVENT_NAME": "workflow_dispatch"}, {"CD_TARGET": "production"},
                           {"CD_DEPLOY_MODE": "bootstrap"}, {"CD_RELEASE_SET": '{"receipts":{"am":{}}}'}):
                with self.subTest(change), patch.dict(os.environ, change):
                    self.assertNotEqual(main(), 0)
            verify_set.assert_not_called()
            timestamp.assert_not_called()

    def test_parallel_component_selection_is_exact(self):
        self.assertEqual(components("am,eg,web"), ["am", "eg", "web"])
        self.assertEqual(components("web"), ["web"])
        for invalid in ("", "am,am", "am,other", "am, eg", "am,eg,web,am", ["am"]):
            with self.subTest(invalid=invalid), self.assertRaises(Refusal):
                components(invalid)

    def test_latest_uses_source_order_and_explicit_versions_need_existing_digest(self):
        old = fixture_receipt()
        new = fixture_receipt(previous="0.10.9", version="0.10.10", source="f" * 40)
        tags = [fixture_tag(old), fixture_tag(new)]
        registry = Mock()
        registry.exists.return_value = True
        registry.manifest.return_value = (old["digest"], {})
        self.assertEqual(resolve("web", "", tags, registry, self.history), new)
        self.assertEqual(resolve("web", "0.10.1", tags, registry, self.history), old)
        for component, version in (("runtime", ""), ("other", ""), ("web", "0.10.2"), ("web", "v0.10.1")):
            with self.subTest(component=component, version=version), self.assertRaises(Refusal):
                resolve(component, version, tags, registry, self.history)
        registry.exists.return_value = False
        with self.assertRaises(Refusal):
            resolve("web", "", tags, registry, self.history)

    def test_automatic_retries_cannot_rollback_over_newer_publication(self):
        old = fixture_receipt()
        new = fixture_receipt(previous="0.10.1", version="0.10.2", source="f" * 40)
        tags = [fixture_tag(old), fixture_tag(new)]
        registry = Mock()
        registry.exists.return_value = True
        registry.manifest.return_value = (old["digest"], {})
        with self.assertRaises(Refusal):
            verify(old, tags, registry, self.history, automatic=True)
        self.assertEqual(verify(new, tags, registry, self.history, automatic=True), new)
        # An explicit maintainer version selection has separate authority.
        self.assertEqual(verify(old, tags, registry, self.history, automatic=False), old)
        with self.assertRaises(Refusal):
            verify(dict(new, digest="sha256:" + "e" * 64), tags, registry, self.history, automatic=False)

    def test_version_manifest_must_agree_with_annotation_for_manual_and_automatic_deploy(self):
        receipt = fixture_receipt()
        registry = Mock()
        registry.exists.return_value = True
        for manifest in (None, ("sha256:" + "f" * 64, {}), (receipt["digest"], {})):
            registry.manifest.return_value = manifest
            for automatic in (False, True):
                with self.subTest(manifest=manifest, automatic=automatic):
                    if manifest is not None and manifest[0] == receipt["digest"]:
                        self.assertEqual(verify(receipt, [fixture_tag(receipt)], registry, self.history,
                                                automatic=automatic), receipt)
                    else:
                        with self.assertRaises(Refusal):
                            verify(receipt, [fixture_tag(receipt)], registry, self.history, automatic=automatic)
            registry.manifest.assert_called_with(receipt["repository"], receipt["identity"]["version"])

    def test_resolver_independently_refuses_unproven_source_before_registry_access(self):
        registry = Mock()
        for source in ("b" * 40, "c" * 40):
            receipt = fixture_receipt(source=source)
            with self.subTest(source=source), self.assertRaises(Refusal):
                resolve("web", "latest", [fixture_tag(receipt)], registry, self.history)
        registry.exists.assert_not_called()
        registry.manifest.assert_not_called()

    def test_dispatch_requires_develop_and_both_actors_are_maintainers(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "event"
            event = {"ref": "refs/heads/develop", "repository": {"full_name": "ChronoAIProject/cma"},
                     "inputs": {"deployables": "web", "version": "", "infra_ref": ""}}
            path.write_text(json.dumps(event))
            env = {"GITHUB_REPOSITORY": "ChronoAIProject/cma", "GITHUB_REF": "refs/heads/develop",
                   "GITHUB_EVENT_NAME": "workflow_dispatch", "GITHUB_SHA": "a" * 40,
                   "GITHUB_EVENT_PATH": str(path), "GITHUB_ACTOR": "fixture-author",
                   "GITHUB_TRIGGERING_ACTOR": "fixture-maintainer"}
            api = Mock(return_value={"role_name": "maintain"})
            self.assertEqual(maintainer_dispatch(env, api), event["inputs"])
            self.assertEqual(api.call_count, 2)
            for role in ("write", "read", "triage", "invented", None):
                with self.subTest(role=role), self.assertRaises(Refusal):
                    maintainer_dispatch(env, Mock(side_effect=[{"role_name": "admin"}, {"role_name": role}]))
            for field, value in (("GITHUB_REF", "refs/heads/main"), ("GITHUB_EVENT_NAME", "pull_request"),
                                 ("GITHUB_REPOSITORY", "Example/fork"), ("GITHUB_ACTOR", "fixture/other")):
                with self.subTest(field=field), self.assertRaises(Refusal):
                    maintainer_dispatch(dict(env, **{field: value}), api)

    def test_push_rejects_deleted_forced_and_mismatched_events(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "event"
            env = dict(os.environ, GITHUB_REPOSITORY="ChronoAIProject/cma", GITHUB_REF="refs/heads/develop",
                       GITHUB_EVENT_NAME="push", GITHUB_SHA="a" * 40, GITHUB_EVENT_PATH=str(path))
            event = {"ref": "refs/heads/develop", "after": "a" * 40, "deleted": False,
                     "repository": {"full_name": "ChronoAIProject/cma"}}
            path.write_text(json.dumps(event))
            self.assertEqual(event_identity(env), event)
            for change in ({"forced": True}, {"deleted": True}, {"after": "b" * 40}, {"repository": None}):
                path.write_text(json.dumps(dict(event, **change)))
                with self.assertRaises(Refusal):
                    event_identity(env)


if __name__ == "__main__":
    unittest.main()
