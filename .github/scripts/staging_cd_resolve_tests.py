"""Manual and automatic redeployment preserve immutable release identity."""
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock

from staging_cd_admission import event_identity, maintainer_dispatch
from staging_cd_common import Refusal
from staging_cd_receipts_tests import fixture_receipt, fixture_tag
from staging_cd_resolve import components, resolve, verify


class ResolveTests(unittest.TestCase):
    def test_parallel_component_selection_is_exact(self):
        self.assertEqual(components("am,eg,web"), ["am", "eg", "web"])
        self.assertEqual(components("web"), ["web"])
        for invalid in ("", "am,am", "am,other", "am, eg", "am,eg,web,am", ["am"]):
            with self.subTest(invalid=invalid), self.assertRaises(Refusal):
                components(invalid)

    def test_latest_uses_numeric_versions_and_explicit_versions_need_existing_digest(self):
        old = fixture_receipt()
        new = fixture_receipt(previous="0.10.9", version="0.10.10", source="f" * 40)
        tags = [fixture_tag(old), fixture_tag(new)]
        registry = Mock()
        registry.exists.return_value = True
        self.assertEqual(resolve("web", "", tags, registry), new)
        self.assertEqual(resolve("web", "0.10.1", tags, registry), old)
        for component, version in (("runtime", ""), ("other", ""), ("web", "0.10.2"), ("web", "v0.10.1")):
            with self.subTest(component=component, version=version), self.assertRaises(Refusal):
                resolve(component, version, tags, registry)
        registry.exists.return_value = False
        with self.assertRaises(Refusal):
            resolve("web", "", tags, registry)

    def test_automatic_retries_cannot_rollback_over_newer_publication(self):
        old = fixture_receipt()
        new = fixture_receipt(previous="0.10.1", version="0.10.2", source="f" * 40)
        tags = [fixture_tag(old), fixture_tag(new)]
        registry = Mock()
        registry.exists.return_value = True
        with self.assertRaises(Refusal):
            verify(old, tags, registry, automatic=True)
        self.assertEqual(verify(new, tags, registry, automatic=True), new)
        # An explicit maintainer version selection has separate authority.
        self.assertEqual(verify(old, tags, registry, automatic=False), old)
        with self.assertRaises(Refusal):
            verify(dict(new, digest="sha256:" + "e" * 64), tags, registry, automatic=False)

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
