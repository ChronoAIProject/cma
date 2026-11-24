from collections import Counter
from pathlib import Path
import sys
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

from repo_secrets import discover_shards, partition, validate_partition


def case(module):
    cls = type("Example", (unittest.TestCase,), {"__module__": module})
    return cls()


class ScannerPartitionTests(unittest.TestCase):
    def setUp(self):
        self.first = case("test_first")
        self.second = case("test_second")
        self.alias = case("tests.test_second")
        self.known = {"test_first", "test_second"}
        self.discovered = [self.second, self.first, self.alias, self.first]

    def test_preserves_order_aliases_and_id_multiplicity(self):
        left, right = partition(self.discovered, self.known)
        self.assertEqual(left, [self.first, self.first])
        self.assertEqual(right, [self.second, self.alias])
        ids = lambda tests: Counter(test.id() for test in tests)
        self.assertFalse(ids(left).keys() & ids(right).keys())
        self.assertEqual(ids(left + right), ids(self.discovered))

    def test_rejects_partition_that_drops_a_module(self):
        extra = case("test_third")
        discovered = [*self.discovered, extra]
        left, right = partition(discovered, self.known | {"test_third"})
        with self.assertRaisesRegex(ValueError, "cover the full test inventory"):
            validate_partition(discovered, ([test for test in left if test is not extra], right))

    def test_rejects_partition_that_drops_one_copy_of_an_id(self):
        left, right = partition(self.discovered, self.known)
        with self.assertRaisesRegex(ValueError, "cover the full test inventory"):
            validate_partition(self.discovered, (left[:1], right))

    def test_rejects_overlapping_ids_even_with_the_right_total_count(self):
        left, right = partition(self.discovered, self.known)
        with self.assertRaisesRegex(ValueError, "must be disjoint"):
            validate_partition(self.discovered, (left[:1], [*right, self.first]))

    def test_rejects_unknown_module(self):
        with self.assertRaisesRegex(ValueError, "unknown scanner test module"):
            partition([*self.discovered, case("test_other")], self.known)

    def test_rejects_missing_module(self):
        with self.assertRaisesRegex(ValueError, "did not cover every frozen test module"):
            partition(self.discovered, self.known | {"test_other"})

    def test_rejects_empty_shard(self):
        with self.assertRaisesRegex(ValueError, "every scanner shard must contain tests"):
            partition([self.first], {"test_first"})

    def test_rejects_discovery_error(self):
        with TemporaryDirectory() as temporary, patch.object(sys, "path", sys.path.copy()):
            bundle = Path(temporary)
            (bundle / "SOURCE-MANIFEST.json").write_text(
                '{"files": [{"path": "tests/test_broken.py"}]}'
            )
            (bundle / "tests").mkdir()
            (bundle / "tests/test_broken.py").write_text("invalid python syntax\n")
            with self.assertRaisesRegex(ValueError, "frozen scanner test discovery failed"):
                discover_shards(bundle)


if __name__ == "__main__":
    unittest.main()
