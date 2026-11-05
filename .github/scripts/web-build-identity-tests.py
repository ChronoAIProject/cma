#!/usr/bin/env python3
"""Regression checks for bundle/OCI identity agreement and placeholder refusal."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("identity", Path(__file__).with_name("web-build-identity.py"))
identity = importlib.util.module_from_spec(spec)
spec.loader.exec_module(identity)


class IdentityTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name)
        self.revision = "0123456789abcdef" * 2 + "01234567"
        self.version = f"git-{self.revision[:12]}"
        self.labels = {
            "org.opencontainers.image.source": "https://github.com/ChronoAIProject/cma",
            "org.opencontainers.image.version": self.version,
            "org.opencontainers.image.revision": self.revision,
        }
        self.evidence = {"version": self.version, "revision": self.revision, "local": False}
        (self.directory / "assets").mkdir()
        self.javascript = self.directory / "assets" / "index-example.js"
        self.javascript.write_text(f'const version="{self.version}", revision="{self.revision}";')

    def verify(self):
        (self.directory / "build-info.json").write_text(json.dumps(self.evidence))
        identity.verify(self.directory, self.labels, self.revision, self.version)

    def test_accept_candidate(self):
        self.verify()

    def test_reject_local_or_mismatched_evidence(self):
        for key, value in [("local", True), ("version", "dev"), ("revision", "development")]:
            with self.subTest(key=key):
                original = self.evidence[key]
                self.evidence[key] = value
                with self.assertRaises(ValueError):
                    self.verify()
                self.evidence[key] = original

    def test_reject_mismatched_labels(self):
        for key in self.labels:
            with self.subTest(key=key):
                original = self.labels[key]
                self.labels[key] = "wrong"
                with self.assertRaises(ValueError):
                    self.verify()
                self.labels[key] = original

    def test_require_identity_in_javascript(self):
        self.javascript.write_text("const version='dev';")
        with self.assertRaisesRegex(ValueError, "compiled JavaScript"):
            self.verify()

    def test_reject_placeholder_inputs(self):
        for version in ["", " ", "dev", "development", "git-aaaaaaaaaaaa"]:
            with self.subTest(version=version), self.assertRaises(ValueError):
                identity.verify(self.directory, self.labels, self.revision, version)


if __name__ == "__main__":
    unittest.main()
