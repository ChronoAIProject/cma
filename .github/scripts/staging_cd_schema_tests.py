"""The published EG schema label comes from its compiled marker contract."""

import json
import os
import contextlib
import io
import eg_schema
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

from eg_schema import LABEL, SOURCE, labels, required_version
from staging_cd_build import build
import staging_cd_build_tests
from staging_cd_reservations import next_reservation


class SchemaTests(unittest.TestCase):
    def test_constant_is_shared_by_verifier_validator_and_migrator(self):
        root = Path(__file__).resolve().parents[2]
        self.assertEqual(required_version(root), 2)
        source = (root / SOURCE).read_text()
        self.assertEqual(source.count('"version":BASE_SCHEMA_VERSION'), 3)
        self.assertNotIn('"_id":"schema","version":2', source)
        self.assertEqual(labels("eg", root), {LABEL: "2"})
        self.assertEqual(labels("cmaeg", root), {LABEL: "2"})
        self.assertEqual(labels("web", root), {})

    def test_missing_duplicate_or_nonliteral_constant_refuses(self):
        for text in ("", "pub const BASE_SCHEMA_VERSION: i32 = 0;",
                     "pub const BASE_SCHEMA_VERSION: i32 = 1 + 1;",
                     "pub const BASE_SCHEMA_VERSION: i32 = 2;\n" * 2):
            with self.subTest(text=text), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                path = root / SOURCE
                path.parent.mkdir(parents=True)
                path.write_text(text)
                with self.assertRaises(ValueError):
                    required_version(root)

    def test_cli_outputs_source_label_and_refuses_missing_source_without_leaking_errors(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / SOURCE
            path.parent.mkdir(parents=True)
            path.write_text("pub const BASE_SCHEMA_VERSION: i32 = 4;\n")
            output = root / "output"
            with patch.object(Path, "cwd", return_value=root), patch.dict(os.environ, {"GITHUB_OUTPUT": str(output)}), \
                    patch("sys.argv", ["eg_schema", "eg"]):
                self.assertEqual(eg_schema.main(), 0)
                self.assertEqual(output.read_text(), LABEL.join(["label=", "=4\n"]))
                path.unlink()
                capture = io.StringIO()
                with contextlib.redirect_stdout(capture):
                    self.assertEqual(eg_schema.main(), 1)
                self.assertEqual(capture.getvalue(), "FAIL schema label\n")
            with patch("sys.argv", ["eg_schema"]), contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(eg_schema.main(), 1)

    def test_candidate_build_uses_its_snapshot_version(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            staging_cd_build_tests.BuildTests().source(root)
            source = root / SOURCE
            source.parent.mkdir(parents=True)
            source.write_text("pub const BASE_SCHEMA_VERSION: i32 = 7;\n")
            dockerfile = root / "api/crates/cmaeg/Dockerfile"
            dockerfile.write_text("FROM scratch AS runtime\n")
            manifest = root / ".github/scripts/deployables.json"
            value = json.loads(manifest.read_text())
            import hashlib

            value["cma-event-gateway"]["dockerfile_sha256"] = hashlib.sha256(
                dockerfile.read_bytes()
            ).hexdigest()
            manifest.write_text(json.dumps(value))

            def run(args, **kwargs):
                (root / "image.oci.tar").write_bytes(b"synthetic image")

            runner = Mock(side_effect=run)
            reservation = next_reservation("eg", "a" * 40, "b" * 40, 42,
                                           "patch", 1, 1, [], [])
            build(root, reservation, root, runner, source=root)
            self.assertIn(LABEL + "=7", runner.call_args.args[0])


if __name__ == "__main__":
    unittest.main()
