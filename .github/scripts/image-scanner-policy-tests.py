#!/usr/bin/env python3
"""A frozen scanner rule file cannot become a general scan bypass."""
import hashlib
import io
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

import image_scanner_policy as policy


class PolicyTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.bundle = self.root / policy.BUNDLE
        self.bundle.mkdir(parents=True)
        (self.bundle / "gitleaks.toml").write_text('[extend]\nuseDefault = true\n')
        (self.bundle / "README.md").write_bytes(b"The scanner disables gitleaks:" + b"allow directives")
        self.manifest = {"schema": "scan-image-secrets-source-v1", "files": [
            {"path": path.name, "sha256": policy.sha256(path)} for path in sorted(self.bundle.iterdir())]}
        self.freeze()
        # The source-policy fixture also carries the independently pinned content bundle.
        scripts = self.root / ".github/scripts"
        for name in (*policy.CONTENT_FILES, "image-content-manifest.json"):
            (scripts / name).parent.mkdir(parents=True, exist_ok=True)
            (scripts / name).write_bytes((Path(__file__).parent / name).read_bytes())
        for relative, pins in policy.package_evidence_pins().values():
            if pins is not None:
                catalog = Path(__file__).parent / relative.name
                (self.root / relative).write_bytes(catalog.read_bytes())

    def freeze(self):
        self.manifest_path = self.bundle / "SOURCE-MANIFEST.json"
        self.manifest_path.write_text(json.dumps(self.manifest))
        self.digest = policy.sha256(self.manifest_path)

    def check(self):
        paths = [p.relative_to(self.root) for p in self.root.rglob("*") if p.is_file()]
        return policy.validate(self.root, paths, self.digest)

    def test_only_exact_frozen_policy_and_documentation_are_allowed(self):
        self.assertEqual(len(self.check()), 3)
        (self.root / "source.py").write_text("print('ordinary source')")
        self.check()

    def test_changed_rule_or_readme_and_additional_bundle_file_fail(self):
        for name in ("gitleaks.toml", "README.md", "extra.py"):
            path = self.bundle / name
            original = path.read_bytes() if path.exists() else None
            path.write_bytes(b"changed bytes")
            with self.subTest(name=name), self.assertRaises(policy.PolicyRefusal):
                self.check()
            if original is None:
                path.unlink()
            else:
                path.write_bytes(original)

    def test_other_configs_and_annotations_fail(self):
        for name, content in (("gitleaks.toml", b"override"), ("nested/.gitleaks.toml", b"override"),
                              (".gitleaksignore", b"fingerprint"), ("source.py", b"gitleaks:" + b"allow")):
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(content)
            with self.subTest(name=name), self.assertRaises(policy.PolicyRefusal):
                self.check()
            path.unlink()

    def test_manifest_substitution_and_unsafe_paths_fail(self):
        self.manifest_path.write_text("{}")
        with self.assertRaises(policy.PolicyRefusal):
            self.check()
        for name in ("../outside", "/absolute"):
            self.manifest["files"].append({"path": name, "sha256": hashlib.sha256(b"").hexdigest()})
            self.freeze()
            with self.subTest(name=name), self.assertRaises(policy.PolicyRefusal):
                self.check()
            self.manifest["files"].pop()

    def test_cli_errors_are_safe(self):
        with patch.object(policy.subprocess, "run", return_value=Mock(stdout=b"file.py\0")), \
             patch.object(policy, "validate") as validate, patch("sys.stdout", new_callable=io.StringIO) as output:
            self.assertEqual(policy.main(), 0)
            validate.side_effect = OSError("private contents")
            self.assertEqual(policy.main(), 1)
            self.assertNotIn("private contents", output.getvalue())

    def test_only_reviewed_packet_bytes_and_review_identity_are_accepted(self):
        digest = "a" * 64
        packet = self.root / policy.CLASSIFICATION_DIR / (digest + ".json")
        packet.parent.mkdir()
        data = {"archive_sha256": digest, "review": {"reviewer": "solution-architect-01", "report_sha256": "b" * 64}}
        packet.write_text(json.dumps(data))
        with self.assertRaises(policy.PolicyRefusal):
            policy.load_classifications(self.root, digest)
        pins = {digest: (policy.sha256(packet), "b" * 64)}
        with patch.object(policy, "APPROVED_CLASSIFICATIONS", pins):
            self.assertEqual(policy.load_classifications(self.root, digest), packet.read_bytes())
            self.check()
            packet.write_text(json.dumps(dict(data, archive_sha256="c" * 64)))
            with self.assertRaises(policy.PolicyRefusal):
                self.check()
            pins[digest] = (policy.sha256(packet), "b" * 64)
            with self.assertRaises(policy.PolicyRefusal):
                self.check()

    def test_missing_untracked_or_symlinked_packets_and_invalid_pins_fail(self):
        digest = "a" * 64
        self.assertIsNone(policy.load_classifications(self.root, digest))
        with self.assertRaises(policy.PolicyRefusal):
            policy.load_classifications(self.root, "../outside")
        with patch.object(policy, "APPROVED_CLASSIFICATIONS", {digest: ("b" * 64, "c" * 64)}), \
             self.assertRaises(policy.PolicyRefusal):
            self.check()
        packet = self.root / policy.CLASSIFICATION_DIR / (digest + ".json")
        packet.parent.mkdir()
        packet.symlink_to(self.root / "absent")
        with self.assertRaises(policy.PolicyRefusal):
            policy.load_classifications(self.root, digest)
        with patch.object(policy, "APPROVED_CLASSIFICATIONS", {digest: ("invalid", "c" * 64)}), \
             self.assertRaises(policy.PolicyRefusal):
            policy.load_classifications(self.root, digest)
        with patch.object(policy, "APPROVED_CLASSIFICATIONS", {digest: ("b" * 64, "c" * 64)}), \
             self.assertRaises(policy.PolicyRefusal):
            policy.load_classifications(self.root, digest)
        packet.unlink()
        packet.write_text('{}')
        with self.assertRaises(policy.PolicyRefusal):
            self.check()

    def test_active_package_evidence_must_match_its_pin_and_be_tracked(self):
        catalog = self.root / policy.PACKAGE_EVIDENCE
        data = b'{"fixture":"reviewed package evidence"}'
        catalog.write_bytes(data)
        pins = (hashlib.sha256(data).hexdigest(), "b" * 64)
        with patch.object(policy, "APPROVED_PACKAGE_EVIDENCE", pins):
            self.check()
            paths = [p.relative_to(self.root) for p in self.root.rglob("*") if p.is_file() and p != catalog]
            with self.assertRaisesRegex(policy.PolicyRefusal, "must be tracked"):
                policy.validate(self.root, paths, self.digest)
            catalog.write_bytes(b'{"fixture":"different bytes"}')
            with self.assertRaisesRegex(policy.PolicyRefusal, "differs from the pinned review"):
                self.check()

    def test_staging_components_load_only_their_reviewed_evidence(self):
        default = self.root / policy.PACKAGE_EVIDENCE
        omarchy = self.root / policy.OMARCHY_PACKAGE_EVIDENCE
        default.write_bytes(b'{"fixture":"runtime"}')
        omarchy.write_bytes(b'{"fixture":"runtime-omarchy"}')
        with patch.object(policy, "APPROVED_PACKAGE_EVIDENCE", (policy.sha256(default), "a" * 64)), \
             patch.object(policy, "APPROVED_OMARCHY_PACKAGE_EVIDENCE", (policy.sha256(omarchy), "b" * 64)):
            self.assertEqual(policy.load_package_evidence(self.root, "runtime")[0], default.read_bytes())
            self.assertEqual(policy.load_package_evidence(self.root, "runtime-omarchy")[0], omarchy.read_bytes())
            self.check()
            omarchy.write_bytes(default.read_bytes())
            with self.assertRaisesRegex(policy.PolicyRefusal, "differs from the pinned review"):
                self.check()

    def test_runtime_package_proofs_match_reserved_build_inputs(self):
        root = Path(__file__).resolve().parents[2]
        checked = set()
        for component, (catalog, _) in policy.package_evidence_pins().items():
            rows = json.loads((root / catalog).read_bytes())["entries"]
            for row in rows:
                for path, expected in row.get("ownership", {}).get("build_inputs", {}).items():
                    if (component, path, expected) in checked:
                        continue
                    checked.add((component, path, expected))
                    with self.subTest(component=component, path=path):
                        self.assertEqual(policy.sha256(root / path), expected)
        self.assertTrue(checked)


if __name__ == "__main__":
    unittest.main()
