"""Large image reports preserve exact file bindings without repeated inventory walks."""

import json
import tempfile
import types
import unittest
from pathlib import Path
from unittest.mock import patch

from image_secrets.backend import Gitleaks
from image_secrets.common import ScanError, digest_file


class BackendIndexTests(unittest.TestCase):
    def test_all_aliases_and_nested_suffixes_bind_exactly(self):
        stage = Path("/private/scan!parent/stage")
        physical = stage / "entry-00001.txt"
        location = {"path": "library.zip", "entry": 1}
        locations = {physical: location}
        lookup = Gitleaks.index_locations(stage, locations)
        for alias in (str(physical), physical.name, str(physical.relative_to(stage))):
            self.assertEqual(Gitleaks.resolve(alias, stage, locations, lookup=lookup), (location, physical))
            resolved, file = Gitleaks.resolve(alias + "!inner!decoded", stage, locations, lookup=lookup)
            self.assertEqual(resolved["path"], "library.zip!inner!decoded")
            self.assertEqual(file, physical)
        for name in (
            "unknown",
            physical.name + "suffix",
            "/outside/" + physical.name,
            "../" + physical.name,
            "",
        ):
            with self.assertRaises(ScanError):
                Gitleaks.resolve(name, stage, locations, lookup=lookup)

    def test_ambiguous_basename_refuses_instead_of_choosing_a_file(self):
        stage = Path("/private/stage")
        with self.assertRaises(ScanError):
            Gitleaks.index_locations(
                stage, {stage / "a/file": {"path": "first"}, stage / "b/file": {"path": "second"}}
            )

    def test_repeated_findings_do_not_walk_the_image_inventory(self):
        stage = Path("/private/stage")
        locations = {stage / ("entry-" + str(i)): {"path": "file-" + str(i)} for i in range(10000)}
        lookup = Gitleaks.index_locations(stage, locations)

        class NoWalk(dict):
            def items(self):
                raise AssertionError("repeated full inventory traversal")

        for _ in range(1000):
            location, physical = Gitleaks.resolve("entry-9999!decoded", stage, NoWalk(), lookup=lookup)
            self.assertEqual(location["path"], "file-9999!decoded")
            self.assertEqual(physical, stage / "entry-9999")

    def test_each_finding_retains_its_value_identity_with_one_file_hash(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            stage = root / "stage"
            stage.mkdir()
            physical = stage / "entry-0.txt"
            physical.write_bytes(b"synthetic public data")
            backend = Gitleaks.__new__(Gitleaks)
            backend.workspace, backend.rules, backend.ignore = root, root / "rules", root / "ignore"
            backend.executable, backend.timeout, backend.env = "gitleaks", 10, {}
            rows = [
                {"File": physical.name, "RuleID": "image-high-entropy", "Secret": candidate, "StartLine": 1}
                for candidate in ("synthetic-one", "synthetic-two")
            ]

            def run(*_args, **_kwargs):
                (root / "gitleaks-private.json").write_text(json.dumps(rows))
                return types.SimpleNamespace(returncode=1)

            records = []
            findings = types.SimpleNamespace(add=lambda *args, **kwargs: records.append((args, kwargs)))
            with (
                patch("image_secrets.backend.subprocess.run", side_effect=run),
                patch("image_secrets.backend.digest_file", wraps=digest_file) as checksum,
            ):
                backend.scan(stage, {physical: {"path": "logical"}}, findings)
                self.assertEqual(checksum.call_count, 1)
            self.assertEqual(len(records), 2)
            self.assertNotEqual(records[0][0][2], records[1][0][2])
            self.assertEqual(records[0][1]["file_digest"], digest_file(physical))
            self.assertEqual(records[0][1], records[1][1])
            self.assertFalse((root / "gitleaks-private.json").exists())


if __name__ == "__main__":
    unittest.main()
