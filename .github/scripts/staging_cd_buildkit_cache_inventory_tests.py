from dataclasses import replace
from datetime import timedelta
import unittest
from unittest.mock import Mock, patch

from actions_cache_buildkit_inventory import CACHE_VERSION, layers, read_index, snapshot
from staging_cd_cache_retention_tests import CLOCK, entry


INDEX = "index-cma-runtime-1-fd3dfe2c#"


def index(number):
    return replace(entry(number, INDEX + str(number)), version=CACHE_VERSION)


def blob(number, digest):
    return replace(entry(number, "buildkit-blob-1-sha256:" + digest * 64), version=CACHE_VERSION)


class BuildKitCacheInventory(unittest.TestCase):
    def test_all_index_generations_protect_their_layers(self):
        current, old = index(1), index(2)
        recent = replace(blob(6, "d"), accessed=CLOCK + timedelta(hours=30))
        referenced, old_ref, unused = blob(3, "a"), blob(4, "b"), blob(5, "c")
        rows = [current, old, referenced, old_ref, unused, recent]
        updated = [replace(current, accessed=recent.accessed), *rows[1:]]
        api = Mock(entries=Mock(side_effect=[rows, updated]))
        refs = {current.key: {referenced.key}, old.key: {old_ref.key}}
        result = snapshot(api, refs.__getitem__, "a" * 40, CLOCK + timedelta(hours=48))
        self.assertEqual((result["index_count"], result["reachable_layer_count"]), (2, 2))
        self.assertEqual([candidate["id"] for candidate in result["candidates"]], [unused.id])
        api.delete.assert_not_called()

    def test_added_generation_invalidates_the_snapshot(self):
        api = Mock(entries=Mock(side_effect=[[index(1)], [index(1), index(2)]]))
        with self.assertRaisesRegex(ValueError, "indexes changed"):
            snapshot(api, lambda key: set(), "a" * 40, CLOCK)

    def test_exact_key_and_download_origin_are_required(self):
        key = INDEX + "1"
        with patch("actions_cache_buildkit_inventory.read_json",
                   return_value={"ok": True, "matched_key": INDEX + "2"}) as read:
            with self.assertRaisesRegex(ValueError, "exact BuildKit index"):
                read_index("https://results-receiver.actions.githubusercontent.com", "fixture", key)
            read.assert_called_once()
        with patch("actions_cache_buildkit_inventory.read_json", return_value={
            "ok": True, "matched_key": key, "signed_download_url": "https://example.invalid/index"
        }) as read:
            with self.assertRaisesRegex(ValueError, "download endpoint"):
                read_index("https://results-receiver.actions.githubusercontent.com", "fixture", key)
            read.assert_called_once()

    def test_unrecognized_layer_formats_fail_the_proof(self):
        self.assertEqual(layers({"layers": [{"blob": "sha256:" + "a" * 64}]}), {blob(1, "a").key})
        for invalid in ({"layers": [{}]}, {"layers": "invalid"}, {"new_schema": 1}):
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                layers(invalid)
