"""Omarchy may classify only the exact reviewed package and sixteen occurrences."""

import copy
import tarfile
import unittest
from unittest.mock import patch

from image_secrets import arch_vectors
from image_secrets.common import ScanError
from image_secrets.key_vectors import PEM, reviewed_vectors, sha
from tests import test_scanner as scanner_tests
from tests.fixtures import docker_archive, fake_key, tar_bytes


class ArchVectorTests(unittest.TestCase):
    tearDown = scanner_tests.ScannerTests.tearDown
    scan = scanner_tests.ScannerTests.scan

    def setUp(self):
        scanner_tests.ScannerTests.setUp(self)
        self.expected = copy.deepcopy(arch_vectors.policy())
        self.entries = []
        for item in self.expected["files"]:
            data = b"synthetic-public-library\0" + b"\0".join(fake_key() for _ in item["blocks"])
            item.update(
                sha256=sha(data),
                bytes=len(data),
                blocks=[
                    dict(offset=m.start(), length=len(m[0]), sha256=sha(m[0])) for m in PEM.finditer(data)
                ],
            )
            self.entries.append((item["path"], data))
        self.expected["metadata"]["mtree_sha256"] = sha(b"synthetic exact package mtree")
        root = arch_vectors.ROOT
        self.entries.extend(
            [
                (root + "/desc", b"%NAME%\ngnutls\n\n%VERSION%\n3.8.13-3\n\n%ARCH%\nx86_64\n"),
                (root + "/files", ("%FILES%\n" + "\n".join(arch_vectors.PATHS) + "\n").encode()),
                (root + "/mtree", b"synthetic exact package mtree"),
                (
                    arch_vectors.HASHES,
                    (
                        self.expected["package"]["archive_sha256"] + "  " + arch_vectors.ARCHIVE + "\n"
                    ).encode(),
                ),
            ]
        )
        self.patcher = patch.object(arch_vectors, "policy", return_value=self.expected)
        self.patcher.start()
        self.addCleanup(self.patcher.stop)

    def image_scan(self, entries, code=1, *, component="runtime-omarchy", earlier=None):
        image = self.root / "image.tar"
        layers = [tar_bytes(entries)]
        if earlier is not None:
            layers.insert(0, tar_bytes(earlier))
        docker_archive(image, layers)
        return self.scan(
            image,
            code,
            ["--blocking-rules=credential", "--reviewed-component=" + component, "--upstream-test-vectors"],
        )[0]

    def test_two_exact_files_retain_sixteen_approved_occurrences(self):
        report = self.image_scan(self.entries, 0)
        self.assertTrue(report["complete"])
        proofs = report["upstream_test_vectors"]
        unique = {proof["file_sha256"]: proof["occurrences"] for proof in proofs}
        self.assertEqual(len(unique), 2)
        self.assertEqual(sum(len(blocks) for blocks in unique.values()), 16)
        self.assertEqual(len(reviewed_vectors(report, "runtime-omarchy")), len(proofs))
        report["policy"]["upstream_test_vectors_sha256"] = "0" * 64
        with self.assertRaises(ScanError):
            reviewed_vectors(report, "runtime-omarchy")

    def test_changed_copied_nested_or_earlier_files_do_not_inherit_authority(self):
        for label in ("alter", "extra", "copy", "path", "nested", "earlier", "component"):
            with self.subTest(case=label):
                entries = list(self.entries)
                path, data = entries[0]
                earlier, component = None, "runtime-omarchy"
                if label == "alter":
                    entries[0] = (path, b"changed" + data)
                if label == "extra":
                    entries[0] = (path, data + fake_key())
                if label == "copy":
                    entries.append(("tmp/copied-library", data))
                if label == "path":
                    entries[0] = ("tmp/copied-library", data)
                if label == "nested":
                    entries.append(("tmp/nested.tar", tar_bytes([(path, data)])))
                if label == "earlier":
                    earlier = [("tmp/earlier-library", data)]
                if label == "component":
                    component = "runtime"
                report = self.image_scan(entries, earlier=earlier, component=component)
                self.assertTrue(report["blocking_findings"])

    def test_linked_file_or_package_ancestors_never_qualify(self):
        for ancestor in ("usr", "usr/lib", "var/lib/pacman", arch_vectors.ROOT):
            with self.subTest(ancestor=ancestor):
                report = self.image_scan(self.entries, earlier=[(ancestor, (tarfile.SYMTYPE, "elsewhere"))])
                self.assertFalse(report["upstream_test_vectors"])

    def test_package_identity_ownership_and_digest_must_all_match(self):
        for index, operation in (
            (2, lambda b: b.replace(b"3.8.13-3", b"3.8.13-4")),
            (2, lambda b: b.replace(b"x86_64", b"aarch64")),
            (3, lambda b: b.replace(b"gnutls-cli", b"unowned-file")),
            (4, lambda b: b + b"changed"),
            (5, lambda b: b"wrong-package-digest"),
        ):
            with self.subTest(entry=index):
                entries = list(self.entries)
                path, data = entries[index]
                entries[index] = (path, operation(data))
                report = self.image_scan(entries)
                self.assertFalse(report["upstream_test_vectors"])
        report = self.image_scan(self.entries[:-1])
        self.assertFalse(report["upstream_test_vectors"])
        report = self.image_scan([*self.entries, self.entries[0]])
        self.assertFalse(report["upstream_test_vectors"])


if __name__ == "__main__":
    unittest.main()
