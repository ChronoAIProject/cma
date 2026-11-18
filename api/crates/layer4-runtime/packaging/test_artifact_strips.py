"""Exact packaging edits preserve runnable members and fail before mutation."""

import base64
import copy
import csv
import io
import tempfile
import unittest
import zipfile
from pathlib import Path

from artifact_edits import sha, transform
from omarchy_artifacts import prepare
from wheel_strips import rebuild


def edit(data=b"keep example keep", start=5, end=12):
    result = data[:start] + b"description" + data[end:]
    return {
        "bytes": len(data),
        "source_sha256": sha(data),
        "result_sha256": sha(result),
        "edits": [
            {
                "start": start,
                "end": end,
                "sha256": sha(data[start:end]),
                "replacement": "description",
            }
        ],
    }


class ArtifactStripTests(unittest.TestCase):
    def test_hash_span_order_and_result_are_required(self):
        data = b"keep example keep"
        self.assertEqual(transform(data, edit()), b"keep description keep")
        for case in ("source", "span", "order", "result"):
            entry = edit()
            if case == "source":
                entry["source_sha256"] = "0" * 64
            if case == "span":
                entry["edits"][0]["end"] += 1
            if case == "order":
                entry["edits"] *= 2
            if case == "result":
                entry["result_sha256"] = "0" * 64
            with self.subTest(case=case), self.assertRaises(ValueError):
                transform(data, entry)

    def wheel(self):
        stream = io.BytesIO()
        with zipfile.ZipFile(stream, "w") as archive:
            archive.writestr("module.py", b"keep example keep")
            archive.writestr("preserved.py", b"def usable(): return 42\n")
            archive.writestr("test-1.dist-info/RECORD", b"original metadata")
        data = stream.getvalue()
        return data, {
            "source_sha256": sha(data),
            "bytes": len(data),
            "members": [dict(edit(), path="module.py")],
        }

    def test_wheel_changes_only_selected_member_and_record(self):
        data, entry = self.wheel()
        result = rebuild(data, entry)
        entry["result_sha256"] = sha(result)
        self.assertEqual(rebuild(data, entry), result)
        with (
            zipfile.ZipFile(io.BytesIO(data)) as before,
            zipfile.ZipFile(io.BytesIO(result)) as after,
        ):
            self.assertEqual(before.namelist(), after.namelist())
            self.assertEqual(before.read("preserved.py"), after.read("preserved.py"))
            for name, digest, length in csv.reader(
                io.StringIO(after.read("test-1.dist-info/RECORD").decode())
            ):
                if name.endswith("/RECORD"):
                    self.assertEqual((digest, length), ("", ""))
                    continue
                actual = after.read(name)
                expected = (
                    base64.urlsafe_b64encode(bytes.fromhex(sha(actual)))
                    .rstrip(b"=")
                    .decode()
                )
                self.assertEqual(digest, "sha256=" + expected)
                self.assertEqual(int(length), len(actual))
            scope = {}
            exec(after.read("preserved.py"), scope)
            self.assertEqual(scope["usable"](), 42)
        for case in ("outer", "member", "result", "missing"):
            changed = copy.deepcopy(entry)
            if case == "outer":
                changed["source_sha256"] = "0" * 64
            if case == "member":
                changed["members"][0]["source_sha256"] = "0" * 64
            if case == "result":
                changed["result_sha256"] = "0" * 64
            if case == "missing":
                changed["members"][0]["path"] = "absent.py"
            with self.subTest(case=case), self.assertRaises(ValueError):
                rebuild(data, changed)

    def test_invalid_source_or_path_cannot_partially_strip(self):
        for case in ("changed", "escape", "symlink", "duplicate"):
            with self.subTest(case=case), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                (root / "remove").write_bytes(b"example")
                (root / "module").write_bytes(b"keep example keep")
                policy = {
                    "schema": "omarchy-artifact-strips-v1",
                    "wheels": [],
                    "remove": [
                        {"path": "remove", "bytes": 7, "source_sha256": sha(b"example")}
                    ],
                    "edit": [dict(edit(), path="module")],
                }
                if case == "changed":
                    (root / "module").write_bytes(b"different")
                if case == "escape":
                    policy["edit"][0]["path"] = "../module"
                if case == "symlink":
                    (root / "module").unlink()
                    (root / "module").symlink_to("remove")
                if case == "duplicate":
                    policy["edit"][0]["path"] = "remove"
                with self.assertRaises(ValueError):
                    prepare(root, policy)
                self.assertEqual((root / "remove").read_bytes(), b"example")


if __name__ == "__main__":
    unittest.main()
