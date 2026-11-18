"""The documentation strip preserves all bytes outside its reviewed paragraph."""

import hashlib
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import npm_placeholder as npm


class NpmPlaceholderTests(unittest.TestCase):
    def test_exact_paragraph_only_and_source_drift_refuses(self):
        data = b"p" * 44957 + b"documentation".ljust(125, b" ") + b"option behavior"
        expected = data[:44957] + data[45082:]
        with patch.multiple(npm, SOURCE=hashlib.sha256(data).hexdigest(),
                            PARAGRAPH=hashlib.sha256(data[44957:45082]).hexdigest(),
                            RESULT=hashlib.sha256(expected).hexdigest()):
            self.assertEqual(npm.stripped(data), expected)
            for changed in (data + b"extra", data[:44957] + b"changed" + data[44964:]):
                with self.assertRaises(ValueError):
                    npm.stripped(changed)

    def test_version_drift_and_redirected_sources_refuse_without_mutation(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for suffix, name, version in (("package.json", "npm", "12.0.2"),
                                          ("node_modules/@npmcli/config/package.json", "@npmcli/config", "wrong")):
                path = root / npm.ROOT / suffix
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(json.dumps({"name": name, "version": version}))
            target = root / npm.FILE
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(b"preserve")
            with self.assertRaises(ValueError):
                npm.prepare(root)
            self.assertEqual(target.read_bytes(), b"preserve")
            target.unlink()
            target.symlink_to(root / npm.ROOT / "package.json")
            with self.assertRaises(ValueError):
                npm.read(root, npm.FILE)


if __name__ == "__main__":
    unittest.main()
