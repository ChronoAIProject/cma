"""Manual runtime selection never inherits application rollback authority."""
import json
import unittest
from unittest.mock import Mock

from staging_cd_common import Refusal
from staging_cd_history import DevelopHistory
from staging_cd_receipts_tests import fixture_receipt, fixture_tag, fixture_runtime_receipt
from staging_cd_resolve import resolve, verify
from staging_cd_set import dispatch_set, release_set, verify_set


class RuntimeOrderTests(unittest.TestCase):
    def fixture(self, component):
        old = fixture_runtime_receipt(component=component, previous="0.10.1", version="0.10.2")
        new = fixture_runtime_receipt(component=component, previous="0.10.2", version="0.10.3", source="f" * 40)
        registry, api = Mock(), Mock()
        registry.exists.return_value = True
        registry.manifest.return_value = (old["digest"], {})
        api.history.return_value = DevelopHistory("f" * 40, ["f" * 40, "a" * 40])
        tags = [fixture_tag(old), fixture_tag(new)]
        api.tags.return_value = tags
        return old, new, registry, api, tags

    def test_manual_old_runtime_refuses_resolution_before_registry_or_writer(self):
        for component in ("runtime", "runtime-omarchy"):
            old, _, registry, api, tags = self.fixture(component)
            writer = Mock()
            with self.assertRaises(Refusal):
                writer(resolve(component, old["identity"]["version"], tags, registry, api.history()))
            writer.assert_not_called()
            registry.exists.assert_not_called()
            api.request.assert_not_called()

    def test_frozen_set_is_rechecked_when_newer_publication_appears(self):
        for component in ("runtime", "runtime-omarchy"):
            for mixed in (False, True):
                old, new, registry, api, tags = self.fixture(component)
                app = fixture_receipt(component="am")
                versions = {component: old["identity"]["version"], **({"am": "0.10.1"} if mixed else {})}
                inputs = {"deployables": ("am," if mixed else "") + component,
                          "versions": json.dumps(versions)}
                api.tags.side_effect = lambda key: [fixture_tag(app)] if key == "am" else [tags[0]]
                _, frozen = dispatch_set(inputs, api, registry)
                api.tags.side_effect = lambda key: [fixture_tag(app)] if key == "am" else tags
                for automatic in (False, True):
                    writer = Mock()
                    with self.assertRaises(Refusal):
                        writer(verify_set(frozen, api, registry, automatic=automatic,
                                          inputs=None if automatic else inputs))
                    writer.assert_not_called()
                api.request.assert_not_called()
                self.assertEqual(verify_set(release_set({component: new}), api, registry, automatic=False,
                                           inputs={"deployables": component, "version": new["identity"]["version"]}),
                                 release_set({component: new}))

    def test_newest_runtime_replay_and_app_manual_rollback_remain_eligible(self):
        for component in ("runtime", "runtime-omarchy"):
            _, new, registry, api, tags = self.fixture(component)
            for automatic in (False, True):
                self.assertEqual(verify(new, tags, registry, api.history(), automatic=automatic), new)
        for component in ("am", "eg", "web"):
            old = fixture_receipt(component=component)
            new = fixture_receipt(component=component, previous="0.10.1", version="0.10.2", source="f" * 40)
            self.assertEqual(verify(old, [fixture_tag(old), fixture_tag(new)], registry,
                                    api.history(), automatic=False), old)
