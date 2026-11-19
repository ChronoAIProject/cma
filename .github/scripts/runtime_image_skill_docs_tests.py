import hashlib
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch


IMAGE = Path(__file__).resolve().parents[2] / "api/crates/layer4-runtime/image"
SPEC = importlib.util.spec_from_file_location("skill_docs", IMAGE / "prepare-skill-docs.py")
DOCS = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(DOCS)


class SkillDocsTests(unittest.TestCase):
    def test_replace_only_pinned_example_and_check_sibling_documents(self):
        digits = b"a" * 64
        response = b'{"refresh_token": "' + digits + b'"}\n'
        for sibling, refuse in ((b'checksum: ' + digits, False),
                                (b'"access_token":\n"' + digits + b'"', True),
                                (b"sessionToken = '" + digits.upper() + b"'", True)):
            with self.subTest(refuse=refuse), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                path = root / "nyxid/references/devices.md"
                path.parent.mkdir(parents=True)
                path.write_bytes(response)
                (root / "README").write_bytes(sibling)
                with patch.object(DOCS, "DEVICES_SHA256", hashlib.sha256(response).hexdigest()):
                    if refuse:
                        with self.assertRaisesRegex(ValueError, "literal token field"):
                            DOCS.prepare(root)
                    else:
                        DOCS.prepare(root)
                        self.assertEqual(path.read_bytes(), b'{"refresh_token": "<refresh_token>"}\n')
                        self.assertEqual((root / "README").read_bytes(), sibling)

    def test_unreviewed_document_refuses_before_writing(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "nyxid/references/devices.md"
            path.parent.mkdir(parents=True)
            path.write_bytes(b"Different documentation")
            with self.assertRaisesRegex(ValueError, "Review changed"):
                DOCS.prepare(tmp)
            self.assertEqual(path.read_bytes(), b"Different documentation")


if __name__ == "__main__":
    unittest.main()
