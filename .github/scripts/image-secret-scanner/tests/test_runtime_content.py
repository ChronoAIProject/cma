"""Reviewed expressions cannot authorize altered files, inputs or occurrences."""

import copy
import io
import json
import os
import subprocess
import tarfile
import unittest
from unittest.mock import patch

from image_secrets import runtime_content as content
from image_secrets import runtime_content_state as state
from image_secrets.common import ScanError
from image_secrets.key_vectors import sha
from tests import test_scanner as scanner_tests
from tests.fixtures import docker_archive, tar_bytes


class RuntimeContentTests(unittest.TestCase):
    setUp = scanner_tests.ScannerTests.setUp
    tearDown = scanner_tests.ScannerTests.tearDown
    scan = scanner_tests.ScannerTests.scan

    def run_image(
        self, entries, files, expected=1, *, earlier=None, component="runtime", architecture="amd64"
    ):
        archive = self.root / "content.tar"
        layers = [tar_bytes(files)]
        if earlier is not None:
            layers.insert(0, tar_bytes(earlier))
        docker_archive(archive, layers, {"architecture": architecture})
        with (
            patch.object(content, "policy", return_value=entries),
            patch.object(state, "policy", return_value=entries),
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

    def fixture(self, kind="dpkg", *, url=False):
        data = b"password = lookup_runtime_value()\n"
        if url:
            data = b"scheme" + b"://name\0part:value\0part@invalid\0"
        entry = {
            "components": ["runtime", "runtime-desktop"],
            "architecture": "amd64",
            "path": "usr/lib/synthetic/module",
            "file_sha256": sha(data),
            "bytes": len(data),
            "detector_input": "file-content",
            "detector_input_sha256": sha(data),
            "package": {"name": "synthetic", "version": "1", "architecture": "amd64"},
            "metadata": {
                "kind": "dpkg",
                "status": "var/lib/dpkg/status",
                "listing": "var/lib/dpkg/info/synthetic:amd64.list",
            },
            "classification": "public-material",
            "match": {},
        }
        status = b"Package: synthetic\nVersion: 1\nArchitecture: amd64\nStatus: install ok installed\n\n"
        files = [
            (entry["path"], data),
            (entry["metadata"]["status"], status),
            (entry["metadata"]["listing"], ("/" + entry["path"] + "\n").encode()),
        ]
        if kind == "npm":
            package = b'{"name":"synthetic","version":"1"}'
            entry["package"].pop("architecture")
            entry["metadata"] = {
                "kind": "npm",
                "path": "usr/lib/synthetic/package.json",
                "sha256": sha(package),
            }
            files = [files[0], (entry["metadata"]["path"], package)]
        baseline = self.run_image([], files)
        rule = "content.credential-url" if url else "gitleaks.image-secret-assignment"
        finding = next(
            f
            for f in baseline["blocking_findings"]
            if f["rule"] == rule and f["path"] == "path-sha256:" + sha(entry["path"].encode())
        )
        entry["match"] = {"rule": rule}
        if url:
            entry["match"]["occurrences"] = content.occurrences(data)
        else:
            entry["match"].update({k: finding[k] for k in ("line", "candidate_sha256")})
            entry["match"]["line_sha256"] = sha(data.splitlines(keepends=True)[finding["line"] - 1])
        return entry, files

    def test_desktop_component_and_exact_all_architecture_package_remain_bound(self):
        entry, files = self.fixture()
        entry["components"] = ["runtime-desktop"]
        entry["package"]["architecture"] = "all"
        changed = [files[0], (files[1][0], files[1][1].replace(b"amd64", b"all")), files[2]]
        self.assertTrue(
            self.run_image([entry], changed, 0, component="runtime-desktop")["reviewed_runtime_content"]
        )
        self.assertFalse(self.run_image([entry], changed)["reviewed_runtime_content"])
        self.assertFalse(
            self.run_image([entry], files, component="runtime-desktop")["reviewed_runtime_content"]
        )
        entry["match"]["line_sha256"] = "0" * 64
        self.assertFalse(
            self.run_image([entry], changed, component="runtime-desktop")["reviewed_runtime_content"]
        )

    def test_exact_file_and_input_preserve_raw_findings_for_dpkg_and_npm(self):
        for kind in ("dpkg", "npm"):
            entry, files = self.fixture(kind)
            result = self.run_image([entry], files, 0)
            self.assertEqual(result["counts"]["blocking"], 0)
            proof = result["reviewed_runtime_content"][0]
            self.assertIn(proof["finding_id"], {f["finding_id"] for f in result["findings"]})
            self.assertEqual(proof["file_sha256"], sha(files[0][1]))
            self.assertEqual(proof["detector_input_sha256"], sha(files[0][1]))

    def test_copied_altered_nested_duplicate_and_redirected_files_remain_blocking(self):
        entry, files = self.fixture()
        for case in ("copy", "alter", "nested", "duplicate", "redirect", "architecture", "component"):
            with self.subTest(case=case):
                changed, earlier, component, architecture = list(files), None, "runtime", "amd64"
                if case == "copy":
                    changed.append(("usr/lib/copied", files[0][1]))
                if case == "alter":
                    changed[0] = (files[0][0], files[0][1] + b"changed")
                if case == "nested":
                    changed.append(("usr/lib/nested.tar", tar_bytes([files[0]])))
                if case == "duplicate":
                    changed.append(files[0])
                if case == "redirect":
                    earlier = [("usr/lib", (tarfile.SYMTYPE, "elsewhere"))]
                if case == "architecture":
                    architecture = "arm64"
                if case == "component":
                    component = "runtime-omarchy"
                result = self.run_image(
                    [entry], changed, earlier=earlier, component=component, architecture=architecture
                )
                self.assertTrue(result["blocking_findings"])

    def test_missing_or_changed_package_name_version_architecture_ownership_refuses(self):
        entry, files = self.fixture()
        variants = [files[:1], [files[0], files[1]], [files[0], files[1], (files[2][0], b"/another/file\n")]]
        for old, new in (
            (b"Version: 1", b"Version: 2"),
            (b"Package: synthetic", b"Package: other"),
            (b"Architecture: amd64", b"Architecture: arm64"),
        ):
            variants.append([files[0], (files[1][0], files[1][1].replace(old, new)), files[2]])
        for changed in variants:
            self.assertFalse(self.run_image([entry], changed)["reviewed_runtime_content"])

    def test_same_path_injected_in_later_layer_cannot_inherit_proof(self):
        entry, files = self.fixture()
        changed = [(files[0][0], files[0][1] + b"password = another_runtime_lookup()\n")]
        result = self.run_image([entry], changed, earlier=files)
        self.assertTrue(result["reviewed_runtime_content"])
        self.assertTrue(any(f["layer"] == 1 for f in result["blocking_findings"]))

    def test_extra_candidate_and_real_key_next_to_reviewed_expression_fail(self):
        entry, files = self.fixture()
        key = subprocess.run(
            ["openssl", "genpkey", "-algorithm", "ED25519"], capture_output=True, check=True, timeout=15
        ).stdout
        for addition in (b"password = another_runtime_lookup()\n", key):
            changed = [(files[0][0], files[0][1] + addition), *files[1:]]
            self.assertFalse(self.run_image([entry], changed)["reviewed_runtime_content"])
        result = self.run_image([entry], [*files, ("usr/lib/other-material", key)])
        self.assertTrue(result["reviewed_runtime_content"])
        self.assertTrue(any(f["rule"] == "content.private-key-header" for f in result["blocking_findings"]))

    def test_coalesced_urls_require_every_occurrence_and_context(self):
        entry, files = self.fixture(url=True)
        result = self.run_image([entry], files, 0)
        self.assertTrue(result["reviewed_runtime_content"])
        for field in ("sha256", "context_sha256", "offset", "length"):
            changed = copy.deepcopy(entry)
            value = changed["match"]["occurrences"][0][field]
            changed["match"]["occurrences"][0][field] = value + 1 if isinstance(value, int) else "0" * 64
            self.assertFalse(self.run_image([changed], files)["reviewed_runtime_content"])
        credential = b"https" + b"://account:" + os.urandom(24).hex().encode() + b"@example.invalid/"
        for data in (files[0][1] + b"\0" + credential, credential + b"\0" + files[0][1]):
            changed = [(files[0][0], data), *files[1:]]
            report = self.run_image([entry], changed)
            self.assertTrue(any(f["rule"] == "content.credential-url" for f in report["blocking_findings"]))
            self.assertFalse(report["reviewed_runtime_content"])

    def test_same_file_independently_failing_detector_is_not_waived(self):
        entry, files = self.fixture()
        result = self.run_image([entry], files, 0)
        from image_secrets.gate import assess

        other = dict(result["findings"][0], rule="gitleaks.provider-secret", finding_id="a" * 64)
        result["findings"].append(other)
        with patch.object(content, "policy", return_value=[entry]):
            assess(result, "credential", component="runtime")
        self.assertIn(other, result["blocking_findings"])

    def test_metadata_binds_real_file_separately_from_detector_input(self):
        original = next(e for e in content.policy() if e["metadata"]["kind"] == "dpkg-control")
        entry = copy.deepcopy(original)
        data = b"synthetic package trigger\n"
        metadata = json.dumps({"path": entry["path"], "link": "", "pax": {}}, ensure_ascii=False).encode()
        entry.update(bytes=len(data), file_sha256=sha(data), detector_input_sha256=sha(metadata))
        package = entry["package"]
        status = (
            "Package: "
            + package["name"]
            + "\nVersion: "
            + package["version"]
            + "\nArchitecture: amd64\nStatus: install ok installed\n\n"
        ).encode()
        files = [(entry["path"], data), (entry["metadata"]["status"], status)]
        report = self.run_image([entry], files, 0)
        proof = report["reviewed_runtime_content"][0]
        self.assertNotEqual(proof["file_sha256"], proof["detector_input_sha256"])
        self.assertFalse(
            content.prove(data, metadata + b"changed", {entry["metadata"]["status"]: status}, entry)
        )
        self.assertFalse(
            content.prove(data + b"changed", metadata, {entry["metadata"]["status"]: status}, entry)
        )
        secret = "runtime_" + os.urandom(24).hex()
        altered = json.dumps({"path": entry["path"], "link": "", "pax": {"password": secret}}).encode()
        self.assertFalse(content.prove(data, altered, {entry["metadata"]["status"]: status}, entry))
        archive = self.root / "metadata-secret.tar"
        payload = io.BytesIO()
        with tarfile.open(fileobj=payload, mode="w", format=tarfile.PAX_FORMAT) as layer:
            for name, body in files:
                member = tarfile.TarInfo(name)
                member.size = len(body)
                if name == entry["path"]:
                    member.pax_headers = {"note": "password=" + secret}
                layer.addfile(member, io.BytesIO(body))
        docker_archive(archive, [payload.getvalue()])
        with (
            patch.object(content, "policy", return_value=[entry]),
            patch.object(state, "policy", return_value=[entry]),
        ):
            result, public = self.scan(
                archive,
                1,
                ["--blocking-rules=credential", "--upstream-test-vectors", "--reviewed-component=runtime"],
            )
        self.assertTrue(result["blocking_findings"])
        self.assertFalse(result["reviewed_runtime_content"])
        self.assertNotIn(secret, public)

    def test_metadata_package_versions_and_release_identity_do_not_float(self):
        entry, files = self.fixture("npm")
        changed = [files[0], (files[1][0], files[1][1].replace(b'"1"', b'"2"'))]
        self.assertFalse(self.run_image([entry], changed)["reviewed_runtime_content"])
        metadata = b"synthetic release version 1\n"
        entry["metadata"] = {"kind": "release-metadata", "path": "usr/lib/version", "sha256": sha(metadata)}
        files = [files[0], (entry["metadata"]["path"], metadata)]
        self.assertTrue(self.run_image([entry], files, 0)["reviewed_runtime_content"])
        changed = [files[0], (files[1][0], b"synthetic release version 2\n")]
        self.assertFalse(self.run_image([entry], changed)["reviewed_runtime_content"])
        entry["metadata"] = {"kind": "release-file"}
        entry["package"] = {"name": "NyxID CLI", "version": "synthetic"}
        self.assertTrue(self.run_image([entry], files[:1], 0)["reviewed_runtime_content"])

    def test_proof_tampering_and_wrong_rule_or_candidate_cannot_classify(self):
        entry, files = self.fixture()
        baseline = self.run_image([entry], files, 0)
        with patch.object(content, "policy", return_value=[entry]):
            for field in ("file_sha256", "detector_input_sha256", "entry_sha256", "policy_sha256"):
                result = copy.deepcopy(baseline)
                result["reviewed_runtime_content"][0][field] = "0" * 64
                with self.assertRaises(ScanError):
                    content.reviewed_runtime_content(result, "runtime")
            for changes in (
                {"candidate_sha256": "0" * 64},
                {"rule": "content.private-key-header"},
                {"member": "0"},
                {"path": "path-sha256:" + "0" * 64},
            ):
                result = copy.deepcopy(baseline)
                result["findings"][0].update(changes)
                with self.assertRaises(ScanError):
                    content.reviewed_runtime_content(result, "runtime")

    def test_registry_rejects_wildcards_duplicate_entries_and_key_rules(self):
        original = json.loads(content.REGISTRY.read_bytes())
        path = self.root / "policy.json"
        for changes in (
            {"path": "../outside"},
            {"file_sha256": "*"},
            {"detector_input_sha256": "*"},
            {"match": {"rule": "content.private-key-header"}},
            {"bytes": True},
        ):
            value = copy.deepcopy(original)
            value["entries"][0].update(changes)
            path.write_text(json.dumps(value))
            with patch.object(content, "REGISTRY", path), self.assertRaises(ScanError):
                content.policy()
        original["entries"].append(original["entries"][0])
        path.write_text(json.dumps(original))
        with patch.object(content, "REGISTRY", path), self.assertRaises(ScanError):
            content.policy()


if __name__ == "__main__":
    unittest.main()
