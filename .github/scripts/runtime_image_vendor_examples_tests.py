"""Test that reviewed examples stay out of the layers that install their modules."""

import hashlib
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch


RUNTIME = Path(__file__).resolve().parents[2] / "api/crates/layer4-runtime"
SPEC = importlib.util.spec_from_file_location("examples", RUNTIME / "image/remove-vendor-examples.py")
EXAMPLES = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(EXAMPLES)


class VendorExampleTests(unittest.TestCase):
    def test_removes_only_reviewed_examples_and_leaves_package_records(self):
        retained = b"module.exports = 1;\n"
        example = b"// Optional example for the package manual.\n"
        original = retained + example
        spec = ("vendor/example.js", hashlib.sha256(original).hexdigest(), (hashlib.sha256(example).hexdigest(),))
        with tempfile.TemporaryDirectory() as temporary, patch.dict(EXAMPLES.EXAMPLES, fixture=spec):
            root = Path(temporary)
            module = root / spec[0]
            module.parent.mkdir()
            module.write_bytes(original)
            package_records = root / "var/lib/dpkg/info"
            package_records.mkdir(parents=True)
            record = package_records / "fixture.list"
            record.write_text("/vendor/example.js\n")

            for _ in range(2):
                EXAMPLES.remove_examples(root, "fixture")
                self.assertEqual(module.read_bytes(), retained)
                self.assertEqual(record.read_text(), "/vendor/example.js\n")

            changed = b"module.exports = 2;\n" + example
            module.write_bytes(changed)
            with self.assertRaisesRegex(ValueError, "requires review"):
                EXAMPLES.remove_examples(root, "fixture")
            self.assertEqual(module.read_bytes(), changed)

    def test_package_installations_remove_examples_before_the_layer_is_written(self):
        instructions = (RUNTIME / "Dockerfile").read_text().replace("\\\n", "").splitlines()
        installer = (RUNTIME / "image/install-os.sh").read_text()
        perl_cleanup = installer.index("python3 -B /tmp/remove-vendor-examples.py perl")
        self.assertLess(installer.index("apt-get upgrade -y"), perl_cleanup)
        self.assertLess(installer.rindex("apt-get install -y"), perl_cleanup)
        for producer in ("sh /tmp/install-os.sh install", "sh /tmp/install-os.sh refresh"):
            with self.subTest(producer=producer):
                [run] = [line for line in instructions if line.startswith("RUN ") and producer in line]
                self.assertIn("source=api/crates/layer4-runtime/image/remove-vendor-examples.py,", run)
        npm_producer = "sh /tmp/install-npm.sh"
        [run] = [line for line in instructions if line.startswith("RUN ") and npm_producer in line]
        self.assertIn("source=api/crates/layer4-runtime/image/remove-vendor-examples.py,", run)
        self.assertIn("python3 -B /tmp/remove-vendor-examples.py npm", run.split(npm_producer, 1)[1])


if __name__ == "__main__":
    unittest.main()
