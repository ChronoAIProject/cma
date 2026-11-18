"""Exact documentation edits fail closed and leave unrelated bytes untouched."""
import copy
import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import documentation as docs


class DocumentationTests(unittest.TestCase):
    def setUp(self):
        self.data = b"functional prefix\n// disposable documentation\nfunctional suffix\n"
        start, end = len(b"functional prefix\n"), self.data.index(b"functional suffix")
        self.result = self.data[:start] + self.data[end:]
        self.entry = {
            "path": "usr/lib/module.js", "bytes": len(self.data),
            "source_sha256": docs.sha(self.data), "result_sha256": docs.sha(self.result),
            "package": {"name": "synthetic", "version": "1"},
            "remove": [{"start": start, "end": end, "sha256": docs.sha(self.data[start:end])}],
        }

    def test_exact_edit_preserves_every_byte_outside_the_reviewed_range(self):
        self.assertEqual(docs.stripped(self.data, self.entry), self.result)

    def test_changed_input_result_or_range_is_refused(self):
        with self.assertRaises(ValueError):
            docs.stripped(b"x" + self.data[1:], self.entry)
        variants = []
        for key in ("result_sha256", "source_sha256"):
            item = copy.deepcopy(self.entry)
            item[key] = "0" * 64
            variants.append(item)
        for delta in ({"start": -1}, {"end": len(self.data) + 1}, {"start": True},
                      {"sha256": "0" * 64}, {"end": 0}):
            item = copy.deepcopy(self.entry)
            item["remove"][0].update(delta)
            variants.append(item)
        item = copy.deepcopy(self.entry)
        item["remove"] *= 2
        variants.append(item)
        for item in variants:
            with self.assertRaises(ValueError):
                docs.stripped(self.data, item)

    def test_all_files_validated_before_writes_and_file_mode_preserved(self):
        with tempfile.TemporaryDirectory() as directory, patch.object(docs, "verify_package"):
            root = Path(directory)
            path = root / self.entry["path"]
            path.parent.mkdir(parents=True)
            path.write_bytes(self.data)
            path.chmod(0o755)
            other = copy.deepcopy(self.entry)
            other["path"] = "usr/lib/other.js"
            (root / other["path"]).write_bytes(b"unexpected")
            policy = {"schema": "runtime-documentation-strips-v1", "entries": [self.entry, other]}
            with self.assertRaises(ValueError):
                docs.prepare(root, policy)
            self.assertEqual(path.read_bytes(), self.data)
            policy["entries"] = [self.entry]
            docs.prepare(root, policy)
            self.assertEqual(path.read_bytes(), self.result)
            self.assertEqual(path.stat().st_mode & 0o777, 0o755)

    def test_unsafe_paths_symlinks_and_oversized_files_are_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = root / "file"
            target.write_bytes(self.data)
            (root / "link").symlink_to(target)
            (root / "directory-link").symlink_to(root, target_is_directory=True)
            for relative in ("../file", "/file", "link", "directory-link/file", "missing", ""):
                with self.assertRaises((ValueError, OSError)):
                    docs.read(root, relative)
            with patch.object(docs, "LIMIT", 1), self.assertRaises(ValueError):
                docs.read(root, "file")

    def test_npm_and_debian_package_drift_refuses(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            data = json.dumps({"name": "synthetic", "version": "1"}).encode()
            (root / "package.json").write_bytes(data)
            entry = copy.deepcopy(self.entry)
            entry["package"].update(metadata_path="package.json", metadata_sha256=docs.sha(data))
            docs.verify_package(root, entry)
            entry["package"]["version"] = "2"
            with self.assertRaises(ValueError):
                docs.verify_package(root, entry)
            entry["package"] = {"name": "synthetic", "version": "1", "architecture": "amd64"}
            with patch.object(docs.subprocess, "run") as run:
                run.return_value = subprocess.CompletedProcess([], 0, "synthetic\n1\namd64\n")
                docs.verify_package(root, entry)
                run.return_value.stdout = "synthetic\n2\namd64\n"
                with self.assertRaises(ValueError):
                    docs.verify_package(root, entry)


if __name__ == "__main__":
    unittest.main()
