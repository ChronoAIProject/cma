import hashlib
import importlib.util
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

IMAGE = Path(__file__).resolve().parents[2] / "api/crates/layer4-runtime/image"
SPEC = importlib.util.spec_from_file_location("skill_docs", IMAGE / "prepare-skill-docs.py")
DOCS = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(DOCS)
PUSH_EXAMPLE = b'  -H "Authorization: Bearer <agent-api-key>" \\\n'
ENV_EXAMPLE = b'  -H "Authorization: Bearer ${NYXID_AGENT_API_KEY:?Set NYXID_AGENT_API_KEY}" \\\n'


def write_references(root, device_data, channel_data=PUSH_EXAMPLE):
    references = root / "nyxid/references"
    references.mkdir(parents=True)
    (references / "devices.md").write_bytes(device_data)
    (references / "channels.md").write_bytes(channel_data)
    return references


class SkillDocsTests(unittest.TestCase):
    def test_replace_only_pinned_example_and_check_sibling_documents(self):
        digits = b"a" * 64
        response = b'{"refresh_token": "' + digits + b'"}\n'
        for sibling, refuse in ((b'checksum: ' + digits, False),
                                (b'| Field | Value |\n|---|---|\n| checksum | `' + digits + b'` |', False),
                                (b'| token | sha256 |\n|---|---|\n| <token> | ' + digits + b' |', False),
                                (b'refresh_token: `' + digits + b'`', True),
                                (b'| Field | Value |\n|---|---|\n| `refresh_token` | `' + digits + b'` |', True),
                                (b'| status | refresh_token |\n|---|---|\n| ok | `' + digits + b'` |', True),
                                (b'"access_token":\n"' + digits + b'"', True),
                                (b"sessionToken = '" + digits.upper() + b"'", True)):
            with self.subTest(refuse=refuse), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                references = write_references(root, response)
                (root / "README").write_bytes(sibling)
                with patch.multiple(DOCS, DEVICES_SHA256=hashlib.sha256(response).hexdigest(),
                                    CHANNELS_SHA256=hashlib.sha256(PUSH_EXAMPLE).hexdigest()):
                    if refuse:
                        with self.assertRaisesRegex(ValueError, "literal token field"):
                            DOCS.prepare(root)
                    else:
                        DOCS.prepare(root)
                        self.assertEqual((references / "devices.md").read_bytes(),
                                         b'{"refresh_token": "<refresh_token>"}\n')
                        self.assertEqual((references / "channels.md").read_bytes(), ENV_EXAMPLE)
                        self.assertEqual((root / "README").read_bytes(), sibling)

    def test_unreviewed_document_refuses_before_writing(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "nyxid/references/devices.md"
            path.parent.mkdir(parents=True)
            path.write_bytes(b"Different documentation")
            with self.assertRaisesRegex(ValueError, "Review changed"):
                DOCS.prepare(tmp)
            self.assertEqual(path.read_bytes(), b"Different documentation")

    def test_escaped_pipe_and_empty_notes_preserve_token_and_checksum_columns(self):
        digest = b"0" * 64
        header = b"| Notes | refresh_token | checksum |\n|---|---|---|\n"
        fixture = b'{"refresh_token": "' + digest + b'"}\n'
        for notes in (rb"one\|two", b"", rb"ends with two backslashes\\"):
            for token, checksum in ((digest, b"ok"), (b"<refresh_token>", digest)):
                with self.subTest(notes=notes, token_literal=token == digest), tempfile.TemporaryDirectory() as tmp:
                    root = Path(tmp)
                    write_references(root, fixture)
                    (root / "table.md").write_bytes(header + b"|" + notes + b"|" + token + b"|" + checksum + b"|\n")
                    with patch.multiple(DOCS, DEVICES_SHA256=hashlib.sha256(fixture).hexdigest(),
                                        CHANNELS_SHA256=hashlib.sha256(PUSH_EXAMPLE).hexdigest()):
                        if token == digest:
                            with self.assertRaisesRegex(ValueError, "literal token field"):
                                DOCS.prepare(root)
                        else:
                            DOCS.prepare(root)

    def test_channels_changes_only_the_shell_header(self):
        prose = b'An example may describe an Authorization header.\n'
        response = b'{"refresh_token": "' + b"0" * 64 + b'"}\n'
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            original = prose + PUSH_EXAMPLE + prose
            references = write_references(root, response, original)
            with patch.multiple(DOCS, DEVICES_SHA256=hashlib.sha256(response).hexdigest(),
                                CHANNELS_SHA256=hashlib.sha256(original).hexdigest()):
                DOCS.prepare(root)
            self.assertEqual((references / "channels.md").read_bytes(), prose + ENV_EXAMPLE + prose)

    def test_channels_refuses_unreviewed_or_ambiguous_input_before_writing(self):
        response = b'{"refresh_token": "' + b"0" * 64 + b'"}\n'
        for channels, pin, error in (
            (PUSH_EXAMPLE + b"revision drift", PUSH_EXAMPLE, "Review changed"),
            (PUSH_EXAMPLE * 2, PUSH_EXAMPLE * 2, "Expected exactly one"),
            (b"No curl example", b"No curl example", "Expected exactly one"),
        ):
            with self.subTest(error=error), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                references = write_references(root, response, channels)
                with (
                    patch.multiple(DOCS, DEVICES_SHA256=hashlib.sha256(response).hexdigest(),
                                   CHANNELS_SHA256=hashlib.sha256(pin).hexdigest()),
                    self.assertRaisesRegex(ValueError, error),
                ):
                    DOCS.prepare(root)
                self.assertEqual((references / "devices.md").read_bytes(), response)
                self.assertEqual((references / "channels.md").read_bytes(), channels)


if __name__ == "__main__":
    unittest.main()
