"""Exact Arch marker proofs cannot authorize key bodies or copied members."""

import base64
import copy
import json
import subprocess
import unittest
from unittest.mock import patch

from image_secrets import marker_policy
from image_secrets.common import ScanError
from image_secrets.key_vectors import sha
from tests import test_markers as marker_tests
from tests.test_ar import ar_bytes


class OmarchyMarkerTests(unittest.TestCase):
    setUp = marker_tests.MarkerTests.setUp
    tearDown = marker_tests.MarkerTests.tearDown
    scan = marker_tests.MarkerTests.scan
    image_scan = marker_tests.MarkerTests.image_scan

    def fixture(self, archive=False):
        entries = [e for e in marker_policy.policy() if e["components"] == ["runtime-omarchy"]]
        entry = copy.deepcopy(
            next(
                e
                for e in entries
                if (e.get("structure", {}).get("kind") == "ar") == archive
                and e["metadata"]["kind"] == "pacman"
            )
        )
        data = b"parser\0-----BEGIN " + b"PRIVATE KEY-----\0recognition-only" + bytes(512)
        if archive:
            member_name = "parser.o"
            outer = ar_bytes([(member_name, data)])
            entry["structure"] = {
                "kind": "ar",
                "member": {
                    "name": member_name,
                    "index": 0,
                    "header_offset": 8,
                    "sha256": sha(data),
                    "bytes": len(data),
                    "marker_offset_in_member": 7,
                },
            }
            data = outer
        entry.update(bytes=len(data), sha256=sha(data), markers=marker_policy.occurrences(data))
        package, metadata = entry["package"], entry["metadata"]
        status = (
            "%NAME%\n" + package["name"] + "\n\n%VERSION%\n" + package["version"] + "\n\n%ARCH%\nx86_64\n"
        ).encode()
        files = [
            (entry["path"], data),
            (metadata["status"], status),
            (metadata["listing"], ("%FILES%\n" + entry["path"] + "\n").encode()),
        ]
        return entry, files

    def check(self, entry, files, code=1, **kwargs):
        return self.image_scan(entry, files, code, component="runtime-omarchy", **kwargs)

    def test_exact_arch_file_and_nested_member_retain_both_raw_findings(self):
        for archive, count in ((False, 1), (True, 2)):
            entry, files = self.fixture(archive)
            result = self.check(entry, files, 0)
            self.assertTrue(result["complete"])
            self.assertEqual(len(result["reviewed_markers"]), count)
            raw = [f for f in result["findings"] if f["rule"] == "content.private-key-header"]
            self.assertEqual(len(raw), count)

    def test_copied_changed_version_missing_and_conflicting_metadata_refuse(self):
        for archive in (False, True):
            entry, files = self.fixture(archive)
            for case in ("copy", "alter", "version", "missing", "duplicate", "conflict", "component"):
                with self.subTest(archive=archive, case=case):
                    changed = list(files)
                    if case == "copy":
                        changed.append(("tmp/copied", files[0][1]))
                    if case == "alter":
                        changed[0] = (files[0][0], b"changed" + files[0][1][7:])
                    if case == "version":
                        changed[1] = (
                            files[1][0],
                            files[1][1].replace(entry["package"]["version"].encode(), b"other"),
                        )
                    if case == "missing":
                        changed.pop()
                    if case == "duplicate":
                        changed.append(files[1])
                    if case == "conflict":
                        changed.append(
                            (
                                files[1][0].rsplit("/", 2)[0]
                                + "/"
                                + entry["package"]["name"]
                                + "-other/desc",
                                files[1][1],
                            )
                        )
                    result = (
                        self.image_scan(entry, changed, component="runtime")
                        if case == "component"
                        else self.check(entry, changed)
                    )
                    self.assertTrue(result["blocking_findings"])

    def test_real_key_bodies_adjacent_nul_distant_encoded_or_reusing_marker_fail(self):
        entry, files = self.fixture()
        key = subprocess.run(
            ["openssl", "genpkey", "-algorithm", "ED25519"], capture_output=True, check=True, timeout=15
        ).stdout
        der = subprocess.run(
            ["openssl", "pkey", "-outform", "DER"], input=key, capture_output=True, check=True, timeout=15
        ).stdout
        encrypted = subprocess.run(
            ["openssl", "pkey", "-aes-256-cbc", "-passout", "pass:synthetic"],
            input=key,
            capture_output=True,
            check=True,
            timeout=15,
        ).stdout
        original = files[0][1]
        body = key[key.index(b"\n") :]
        marker_end = marker_policy.HEADER.search(original).end()
        reuse = original[:marker_end] + body
        reuse += bytes(len(original) - len(reuse))
        for data in (
            original + key,
            original + b"\0" + key,
            original + bytes(33000) + key,
            original + der,
            original + encrypted,
            original + base64.b64encode(key),
            original + json.dumps(key.decode()).encode(),
            reuse,
        ):
            result = self.check(entry, [(entry["path"], data), *files[1:]])
            self.assertFalse(result["reviewed_markers"])
            self.assertTrue(result["blocking_findings"])

    def test_copied_member_and_changed_parent_cannot_inherit_ar_proof(self):
        entry, files = self.fixture(True)
        content = files[0][1][68 : 68 + entry["structure"]["member"]["bytes"]]
        result = self.check(entry, [*files, ("tmp/parser.o", content)])
        self.assertTrue(result["blocking_findings"])
        changed = ar_bytes([("parser.o", content), ("extra.o", b"additional")])
        result = self.check(entry, [(entry["path"], changed), *files[1:]])
        self.assertFalse(result["reviewed_markers"])
        baseline = self.check(entry, files, 0)
        with patch.object(marker_policy, "policy", return_value=[entry]):
            for changes in ({"member": "1"}, {"member": 0}, {"path": "path-sha256:" + "0" * 64}):
                result = copy.deepcopy(baseline)
                next(f for f in result["findings"] if "member" in f).update(changes)
                with self.assertRaises(ScanError):
                    marker_policy.reviewed_markers(result, "runtime-omarchy")


if __name__ == "__main__":
    unittest.main()
