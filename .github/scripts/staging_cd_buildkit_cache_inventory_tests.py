from collections import Counter
from dataclasses import replace
from datetime import timedelta
from io import BytesIO
import unittest
from unittest.mock import Mock, patch

from actions_cache_buildkit_inventory import BLOB_PREFIX, CACHE_VERSION, INDEX_PREFIX, layers, read_index, read_json, snapshot
from actions_cache_retention import DEVELOP
from staging_cd_cache_retention_tests import CLOCK, entry


INDEX = "index-cma-runtime-1-fd3dfe2c#"


def index(number):
    return replace(entry(number, INDEX + str(number)), version=CACHE_VERSION)


def blob(number, digest):
    return replace(entry(number, "buildkit-blob-1-sha256:" + digest * 64), version=CACHE_VERSION)


def inventory(before, after):
    calls = Counter()

    def entries(*, key, sort, direction, ref=None):
        observation = before if calls[key] == 0 else after
        calls[key] += 1
        if key == INDEX_PREFIX:
            if ref is not None:
                raise AssertionError("index lookup must span all refs")
            return [entry for entry in observation if entry.key.startswith(key)]
        if ref != DEVELOP or key[:-1] != BLOB_PREFIX:
            raise AssertionError("blob lookup must use the develop ref and a digest partition")
        return [entry for entry in observation if entry.key.startswith(key) and entry.ref == ref]

    return Mock(entries=Mock(side_effect=entries))


class BuildKitCacheInventory(unittest.TestCase):
    def test_all_index_generations_protect_their_layers(self):
        current, old = index(1), index(2)
        recent = replace(blob(6, "d"), accessed=CLOCK + timedelta(hours=30))
        referenced, old_ref, unused = blob(3, "a"), blob(4, "b"), blob(5, "c")
        accessed_during_inventory = blob(7, "e")
        rows = [current, old, referenced, old_ref, unused, recent, accessed_during_inventory]
        updated = [replace(current, accessed=recent.accessed), *rows[1:-1],
                   replace(accessed_during_inventory, accessed=CLOCK + timedelta(hours=1))]
        api = inventory(rows, updated)
        refs = {current.key: {referenced.key}, old.key: {old_ref.key}}
        reader = Mock(side_effect=lambda key, size: refs[key])
        result = snapshot(api, reader, "a" * 40, CLOCK + timedelta(hours=48))
        reader.assert_any_call(current.key, current.size)
        self.assertEqual((result["index_count"], result["reachable_layer_count"]), (2, 2))
        self.assertEqual(result["observed_develop_blob_bytes"], sum(entry.size for entry in rows[2:]))
        self.assertEqual([candidate["id"] for candidate in result["candidates"]], [unused.id])
        api.delete.assert_not_called()

    def test_added_generation_invalidates_the_snapshot(self):
        api = inventory([index(1)], [index(1), index(2)])
        with self.assertRaisesRegex(ValueError, "indexes changed"):
            snapshot(api, lambda key, size: set(), "a" * 40, CLOCK)

    def test_retirement_projection_preserves_cross_scope_and_two_latest_references(self):
        old, spare, current = (index(value) for value in (8, 9, 10))
        elsewhere = replace(index(11), key=INDEX.replace("cma-runtime", "unmanaged") + "11")
        old_only, warm, backup, shared = (blob(i, letter) for i, letter in enumerate("abcd", start=20))
        rows = [old, spare, current, elsewhere, old_only, warm, backup, shared]
        refs = {old.key: {old_only.key, shared.key}, spare.key: {backup.key},
                current.key: {warm.key}, elsewhere.key: {shared.key}}
        api = inventory(rows, rows)
        result = snapshot(api, lambda key, _: refs[key], "a" * 40, CLOCK + timedelta(hours=48))
        self.assertEqual(result["candidate_bytes"], 0)
        self.assertEqual(result["superseded_index_count"], 1)
        self.assertEqual(result["projected_candidate_bytes_after_index_retirement"], old_only.size)
        self.assertEqual(result["next_index_batch_count"], 1)
        self.assertEqual(result["projected_newly_unreachable_bytes"], old_only.size)
        api.delete.assert_not_called()

    def test_unordered_or_incomplete_replacements_hold_the_scope(self):
        old, spare, current = (index(value) for value in (8, 9, 10))
        old_only, warm = blob(20, "a"), blob(21, "b")
        refs = {old.key: {old_only.key}, spare.key: {warm.key}, current.key: {warm.key}}
        for last, replacement_blobs in (
            (current, []),
            (replace(current, created=old.created - timedelta(seconds=1)), [warm]),
        ):
            with self.subTest(last=last, replacement_blobs=replacement_blobs):
                rows = [old, spare, last, old_only, *replacement_blobs]
                result = snapshot(inventory(rows, rows), lambda key, _: refs[key],
                                  "a" * 40, CLOCK + timedelta(hours=48))
                self.assertEqual(result["superseded_index_count"], 0)
                self.assertEqual(result["projected_candidate_bytes_after_index_retirement"], 0)

    def test_exact_key_and_download_origin_are_required(self):
        key = INDEX + "1"
        with patch("actions_cache_buildkit_inventory.read_json",
                   return_value={"ok": True, "matched_key": INDEX + "2"}) as read:
            with self.assertRaisesRegex(ValueError, "exact BuildKit index"):
                read_index("https://results-receiver.actions.githubusercontent.com", "fixture", key, 100)
            read.assert_called_once()
        with patch("actions_cache_buildkit_inventory.read_json", return_value={
            "ok": True, "matched_key": key, "signed_download_url": "https://example.invalid/index"
        }) as read:
            with self.assertRaisesRegex(ValueError, "download endpoint"):
                read_index("https://results-receiver.actions.githubusercontent.com", "fixture", key, 100)
            read.assert_called_once()

    def test_unrecognized_layer_formats_fail_the_proof(self):
        sample = {"layers": [{"blob": "sha256:" + "a" * 64, "parent": -1}],
                  "records": [{"layers": [{}], "chains": [{"layers": [0]}]}]}
        self.assertEqual(layers(sample), {blob(1, "a").key})
        self.assertEqual(layers({}), set())
        for invalid in ({"layers": [{}]}, {"layers": "invalid"}, {"new_schema": 1},
                        {"records": [{"layers": [{}]}]}, {"records": [{"chains": [{"layers": [1]}]}]},
                        {"layers": [{"blob": "sha256:" + "a" * 64, "Blob": "sha256:" + "b" * 64}]}):
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                layers(invalid)

    def test_download_size_and_unambiguous_json(self):
        with patch("actions_cache_buildkit_inventory.build_opener") as opener:
            opener.return_value.open.return_value = BytesIO(b"{}")
            with self.assertRaisesRegex(ValueError, "inventory size"):
                read_json("https://fixture.invalid", 100, expected_size=3)
            content = b'{"layers":[{"blob":"a","Blob":"b"}]}'
            opener.return_value.open.return_value = BytesIO(content)
            with self.assertRaisesRegex(ValueError, "ambiguous"):
                read_json("https://fixture.invalid", 100, expected_size=len(content))
