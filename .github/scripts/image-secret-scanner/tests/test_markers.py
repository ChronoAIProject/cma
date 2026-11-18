"""Reviewed parser literals never grant permission to ship adjacent key bytes."""

import copy
import json
import subprocess
import tarfile
import unittest
from unittest.mock import patch

from image_secrets import marker_policy, marker_state
from image_secrets.common import ScanError
from image_secrets.key_vectors import sha
from tests import test_scanner as scanner_tests
from tests.fixtures import docker_archive, tar_bytes


class MarkerTests(unittest.TestCase):
    setUp = scanner_tests.ScannerTests.setUp
    tearDown = scanner_tests.ScannerTests.tearDown
    scan = scanner_tests.ScannerTests.scan

    def fixture(self, kind="dpkg"):
        entry = copy.deepcopy(next(e for e in marker_policy.policy() if e["metadata"]["kind"] == kind))
        data = b"comparison\0-----BEGIN " + b"OPENSSH PRIVATE KEY-----\0parser-format\0"
        if kind == "npm":
            data = b"example: -----BEGIN " + b"PRIVATE KEY-----\\nXXXX\\n-----END PRIVATE KEY-----"
        entry.update(bytes=len(data), sha256=sha(data), markers=marker_policy.occurrences(data))
        files = [(entry["path"], data)]
        metadata, package = entry["metadata"], entry["package"]
        if kind == "dpkg":
            status = (
                "Package: "
                + package["name"]
                + "\nVersion: "
                + package["version"]
                + "\nArchitecture: amd64\nStatus: install ok installed\n\n"
            ).encode()
            files.extend(
                [(metadata["status"], status), (metadata["listing"], ("/" + entry["path"] + "\n").encode())]
            )
        elif kind == "npm":
            files.append(
                (metadata["path"], json.dumps({k: package[k] for k in ("name", "version")}).encode())
            )
        return entry, files

    def image_scan(
        self, entry, files, expected=1, *, earlier=None, component="runtime", architecture="amd64"
    ):
        archive = self.root / "image.tar"
        layers = [tar_bytes(files)]
        if earlier is not None:
            layers.insert(0, tar_bytes(earlier))
        docker_archive(archive, layers, {"architecture": architecture})
        with (
            patch.object(marker_state, "policy", return_value=[entry]),
            patch.object(marker_policy, "policy", return_value=[entry]),
        ):
            return self.scan(
                archive,
                expected,
                [
                    "--blocking-rules=credential",
                    "--upstream-test-vectors",
                    "--reviewed-component=" + component,
                ],
            )[0]

    def test_exact_marker_and_npm_placeholder_retain_raw_detection(self):
        for kind in ("dpkg", "npm", "release-file"):
            with self.subTest(kind=kind):
                entry, files = self.fixture(kind)
                result = self.image_scan(entry, files, 0)
                self.assertTrue(result["complete"])
                self.assertEqual(len(result["reviewed_markers"]), 1)
                self.assertTrue(any(f["rule"] == "content.private-key-header" for f in result["findings"]))
                self.assertEqual(
                    result["reviewed_classifications"][0]["classification"], "reviewed-non-key-marker"
                )

    def test_real_generated_key_next_to_marker_always_blocks(self):
        entry, files = self.fixture()
        key = subprocess.run(
            ["openssl", "genpkey", "-algorithm", "ED25519"], capture_output=True, check=True, timeout=15
        ).stdout
        for separate in (False, True):
            changed = list(files)
            if separate:
                changed.append(("tmp/unapproved-material", key))
            else:
                changed[0] = (files[0][0], files[0][1] + key)
            result = self.image_scan(entry, changed)
            self.assertTrue(result["blocking_findings"])
            if not separate:
                self.assertFalse(result["reviewed_markers"])

    def test_changed_copied_nested_or_wrong_platform_and_component_refuse(self):
        entry, files = self.fixture()
        for case in (
            "alter",
            "copy",
            "nested",
            "history",
            "architecture",
            "component",
            "ancestor",
            "duplicate",
        ):
            with self.subTest(case=case):
                changed, earlier, component, architecture = list(files), None, "runtime", "amd64"
                if case == "alter":
                    changed[0] = (files[0][0], files[0][1] + b"changed")
                if case == "copy":
                    changed.append(("tmp/copied", files[0][1]))
                if case == "nested":
                    changed.append(("tmp/nested.tar", tar_bytes([files[0]])))
                if case == "history":
                    earlier = [("tmp/copied", files[0][1])]
                if case == "architecture":
                    architecture = "arm64"
                if case == "component":
                    component = "runtime-omarchy"
                if case == "ancestor":
                    earlier = [("usr/lib", (tarfile.SYMTYPE, "elsewhere"))]
                if case == "duplicate":
                    changed.append(files[0])
                result = self.image_scan(
                    entry, changed, earlier=earlier, component=component, architecture=architecture
                )
                self.assertTrue(result["blocking_findings"])

    def test_version_and_ownership_must_match_for_package_and_npm(self):
        for kind in ("dpkg", "npm"):
            entry, files = self.fixture(kind)
            changed = list(files)
            changed[1] = (
                files[1][0],
                files[1][1].replace(entry["package"]["version"].encode(), b"unreviewed-version"),
            )
            self.assertFalse(self.image_scan(entry, changed)["reviewed_markers"])
            self.assertFalse(self.image_scan(entry, files[:1])["reviewed_markers"])
            changed = list(files)
            changed[0] = (
                files[0][0],
                files[0][1].replace(b"XXXX", b"YYYY") if kind == "npm" else files[0][1] + b"\0",
            )
            self.assertFalse(self.image_scan(entry, changed)["reviewed_markers"])

    def test_report_proof_cannot_waive_another_rule_or_location(self):
        entry, files = self.fixture()
        baseline = self.image_scan(entry, files, 0)
        with patch.object(marker_policy, "policy", return_value=[entry]):
            for changes in (
                {"rule": "gitleaks.private-key"},
                {"scope": "config"},
                {"path": "path-sha256:" + "0" * 64},
                {"member": 0},
            ):
                result = copy.deepcopy(baseline)
                finding = next(f for f in result["findings"] if f["rule"] == "content.private-key-header")
                finding.update(changes)
                with self.assertRaises(ScanError):
                    marker_policy.reviewed_markers(result, "runtime")
            result = copy.deepcopy(baseline)
            result["reviewed_markers"][0]["file_sha256"] = "0" * 64
            with self.assertRaises(ScanError):
                marker_policy.reviewed_markers(result, "runtime")


if __name__ == "__main__":
    unittest.main()
