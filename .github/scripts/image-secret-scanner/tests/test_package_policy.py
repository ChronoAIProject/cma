"""Exercise the reviewed package evidence path on synthetic, fully scanned images."""

import copy
import gzip
import io
import json
import stat
import subprocess
import sys
import tarfile
import unittest
import zipfile
from pathlib import Path
from unittest.mock import patch

from image_secrets.common import ScanError, digest_bytes
from image_secrets.gate import assess
from image_secrets.package_evidence import PackageEvidence
from image_secrets.package_occurrences import prove_package_member
from image_secrets.package_receipt import build, decisions
from tests.fixtures import CAPABILITY_XATTR, docker_archive, tar_bytes
from tests.test_runtime_archives import ar_bytes
from tests.test_scanner import ScannerTests

SOURCE = "usr/lib/example/parser.js"
SYMBOL = b"readPasswordFromRequestInput"
PUBLIC_SOURCE = b"const password = " + SYMBOL + b"();\n"
RULE = "gitleaks.image-secret-assignment"
SCANNER = "1" * 64
REVIEW = "2" * 64


class PackageTests(unittest.TestCase):
    setUp = ScannerTests.setUp
    tearDown = ScannerTests.tearDown
    scan = ScannerTests.scan

    def test_native_member_proof_refuses_private_binary_xattr(self):
        key = subprocess.run(["openssl", "genpkey", "-algorithm", "EC", "-pkeyopt",
                              "ec_paramgen_curve:prime256v1", "-outform", "DER"],
                             check=True, capture_output=True, timeout=15).stdout
        attrs = {SOURCE: {CAPABILITY_XATTR: key.decode("utf-8", "surrogateescape")}}
        artifact = self.root / "payload.tar"
        payload = tar_bytes([(SOURCE, PUBLIC_SOURCE)], pax_headers=attrs)
        artifact.write_bytes(payload)
        with self.assertRaises(ScanError):
            prove_package_member(artifact, digest_bytes(payload), "0", self.root)

    def manifest(self, supplied, layer, *, component="runtime", file=SOURCE, member=None, public=PUBLIC_SOURCE):
        # The fixture's vendor archive is the trusted input, outside the image.
        trusted = digest_bytes(supplied)
        vendor = self.root / "vendor-package"
        vendor.write_bytes(supplied)
        package_member = prove_package_member(vendor, trusted, "0", self.root)
        self.assertEqual(package_member["path"], [file])
        return {
            "schema": "cma-staging-package-evidence-v1",
            "scanner_manifest_sha256": SCANNER,
            "packages": [{"name": "example-parser", "version": "1.0", "architecture": "all",
                          "artifact_sha256": trusted, "authority_type": "pinned-upstream",
                          "authority_sha256": trusted}],
            "entries": [{"component": component, "platform": "linux/amd64", "layer": 0,
                         "diff_id": "sha256:" + digest_bytes(layer), "entry": 0, "path": file,
                         "file_sha256": package_member["sha256"], "package_sha256": trusted,
                         "package_member": package_member, "member": member,
                         "matches": [{"rule": RULE, "candidate_sha256": digest_bytes(SYMBOL),
                                      "line": 1, "explanation": "caller-supplied-value-expression",
                                      "proof_sha256": digest_bytes(public)}]}],
        }

    @staticmethod
    def policy(manifest, expected=None):
        data = json.dumps(manifest).encode()
        return PackageEvidence.load(data, manifest_sha256=expected or digest_bytes(data),
                                    review_sha256=REVIEW, scanner_manifest_sha256=SCANNER)

    def classify(self, archive, report, evidence, component="runtime"):
        result = build(archive, self.root, report, component, evidence)
        assessed = copy.deepcopy(report)
        assess(assessed, "credential", verified_package_decisions=decisions(result))
        self.assertEqual(assessed["findings"], report["findings"])
        return result, assessed

    def detector_scan(self, archive, component="runtime", extra=()):
        options = ["--blocking-rules", "credential", "--staging-runtime-component", component, *extra]
        return self.scan(archive, 1, options)[0]

    def test_verified_public_expression_and_private_key_in_same_distribution(self):
        ephemeral = subprocess.run(["openssl", "genpkey", "-algorithm", "RSA", "-pkeyopt", "rsa_keygen_bits:2048"],
                                   timeout=30, check=True, capture_output=True).stdout
        entries = [(SOURCE, PUBLIC_SOURCE), ("usr/share/example/material", ephemeral)]
        supplied = tar_bytes(entries)
        archive = self.root / "image.tar"
        docker_archive(archive, [supplied])
        report = self.detector_scan(archive)
        section, final = self.classify(archive, report, self.policy(self.manifest(supplied, supplied)))
        self.assertEqual(len(section["entries"]), 1)
        self.assertEqual(section["entries"][0]["package"]["artifact_sha256"], digest_bytes(supplied))
        self.assertIn("content.private-key-header", {f["rule"] for f in final["blocking_findings"]})
        self.assertNotIn(SYMBOL.decode(), json.dumps(section))

    def test_earlier_bytes_and_app_owned_overwrite_have_independent_decisions(self):
        supplied = tar_bytes([(SOURCE, PUBLIC_SOURCE)])
        # A separate image-owned layer has no reviewed provenance, even for identical bytes.
        copied = tar_bytes([(SOURCE, PUBLIC_SOURCE), ("usr/lib/example/app-owned", b"ordinary file")])
        archive = self.root / "image.tar"
        docker_archive(archive, [supplied, copied])
        raw = self.detector_scan(archive)
        section, final = self.classify(archive, raw, self.policy(self.manifest(supplied, supplied)))
        self.assertEqual([row["layer"] for row in section["entries"]], [0])
        self.assertIn((1, RULE), {(row["layer"], row["rule"]) for row in final["blocking_findings"]})
        forged = tar_bytes([(SOURCE, PUBLIC_SOURCE + b"// locally modified\n")])
        docker_archive(archive, [forged, supplied])
        new = self.detector_scan(archive)
        section, _ = self.classify(archive, new, self.policy(self.manifest(supplied, supplied)))
        self.assertEqual(section["entries"], [])

    def test_additional_credential_and_wrong_package_or_bytes_remain_blocking(self):
        supplied = tar_bytes([(SOURCE, PUBLIC_SOURCE)])
        unrelated = b"password = " + b"FixtureOnlyValueForThisRegression42" + b"\n"
        changed = tar_bytes([(SOURCE, PUBLIC_SOURCE + unrelated)])
        archive = self.root / "image.tar"
        docker_archive(archive, [changed])
        report = self.detector_scan(archive)
        # Even an otherwise approved layer locator cannot replace the trusted member bytes.
        raw = self.manifest(supplied, changed)
        section, final = self.classify(archive, report, self.policy(raw))
        self.assertEqual(section["entries"], [])
        self.assertEqual(final["blocking_findings"], report["blocking_findings"])
        original = digest_bytes(json.dumps(raw).encode())
        raw["packages"][0]["artifact_sha256"] = "f" * 64
        with self.assertRaises(ScanError):
            self.policy(raw, expected=original)
        with self.assertRaises(ScanError):
            self.policy(raw)

    def test_nested_match_binds_outer_and_member_in_both_cache_scopes(self):
        path = "usr/share/example/package.tar"
        nested = tar_bytes([("source.js", PUBLIC_SOURCE)])
        supplied = tar_bytes([(path, nested)])
        member = {"locator": "0", "path": ["source.js"], "sha256": digest_bytes(PUBLIC_SOURCE)}
        archive = self.root / "image.tar"
        docker_archive(archive, [supplied])
        cache = self.root / "detector-cache.json"
        for component in ("runtime", "runtime-omarchy"):
            evidence = self.policy(self.manifest(supplied, supplied, component=component, file=path, member=member))
            options = ["--detector-cache-context", "a" * 64, "--detector-cache-output", str(cache)]
            cold = self.detector_scan(archive, component, options)
            warm = self.detector_scan(archive, component, [*options, "--detector-cache-input", str(cache)])
            self.assertEqual(warm["detector_cache"]["hits"], 1)
            cold_section, cold_final = self.classify(archive, cold, evidence, component)
            warm_section, warm_final = self.classify(archive, warm, evidence, component)
            self.assertEqual(cold_final["counts"], warm_final["counts"])
            self.assertEqual(cold_section, warm_section)
            self.assertEqual(len(cold_section["entries"]), 1)
            self.assertEqual(cold_section["entries"][0]["outer_file_sha256"], digest_bytes(nested))
            self.assertEqual(cold_section["entries"][0]["member"], "0")
            other_component = "runtime-omarchy" if component == "runtime" else "runtime"
            self.assertEqual(self.classify(archive, self.detector_scan(archive, other_component),
                                           evidence, other_component)[0]["entries"], [])

    def test_private_openpgp_packets_block_even_without_a_keyring_path(self):
        # Reserved fixture bytes exercise packet framing without storing a key.
        key_body = bytes([4, 0, 0, 0, 0, 1, 0, 0])
        public = bytes([0xC6, len(key_body)]) + key_body
        old_private = bytes([0x94, len(key_body)]) + key_body
        private_subkey = bytes([0xC7, len(key_body)]) + key_body
        archive = self.root / "image.tar"
        layer = tar_bytes([("ordinary/a", public), ("ordinary/b", old_private),
                           ("ordinary/c", public + private_subkey)])
        docker_archive(archive, [layer])
        report = self.detector_scan(archive)
        matches = {row["entry"] for row in report["blocking_findings"]
                   if row["rule"] == "content.openpgp-private-key"}
        self.assertEqual(matches, {1, 2})

    def test_openpgp_markers_preserve_public_and_secret_key_decisions(self):
        key_body = bytes([4, 0, 0, 0, 0, 1, 0, 0])
        marker = bytes([0xCA, 3]) + b"PGP"
        public = bytes([0xC6, len(key_body)]) + key_body
        secret = bytes([0xC5, len(key_body)]) + key_body
        subkey = bytes([0xC7, len(key_body)]) + key_body
        non_critical = bytes([0xE8, 1, 0])
        archive = self.root / "marker-keyrings.tar"
        entries = [("keys/a", marker + marker + public), ("keys/b", marker + secret),
                   ("keys/c", public + marker + subkey), ("keys/d", marker),
                   ("keys/e", non_critical + public), ("keys/f", non_critical + marker + secret)]
        docker_archive(archive, [tar_bytes(entries)])
        report = self.detector_scan(archive)
        matches = {row["entry"] for row in report["blocking_findings"]
                   if row["rule"] == "content.openpgp-private-key"}
        self.assertEqual(matches, {1, 2, 5})

    def test_publication_gate_recomputes_package_section_and_retains_full_findings(self):
        scripts = Path(__file__).resolve().parents[2]
        archive = self.root / "publish.tar"
        supplied = tar_bytes([(SOURCE, PUBLIC_SOURCE)])
        machine_identity = [("etc/machine-id", b""),
                            ("var/lib/dbus/machine-id", (tarfile.SYMTYPE, "/etc/machine-id"))]
        layer = tar_bytes([(SOURCE, PUBLIC_SOURCE), *machine_identity])
        docker_archive(archive, [layer])
        with patch.object(sys, "path", [str(scripts), *sys.path]):
            import image_secret_gate as gate

            manifest = self.manifest(supplied, layer)
            manifest["scanner_manifest_sha256"] = gate.MANIFEST_SHA256
            raw = json.dumps(manifest).encode()
            evidence = raw, digest_bytes(raw), REVIEW
            output = self.root / "publish-scan"
            with patch.object(gate, "load_package_evidence", return_value=evidence):
                actual, scanner = gate.verified_staging_scan(archive, output, component="runtime")
            receipt = json.loads((output / "REPORT.json").read_bytes())
            section = receipt["package_evidence"]
            self.assertEqual(scanner, ("1.0.4", gate.MANIFEST_SHA256))
            self.assertEqual(receipt["counts"], {"detected": 1, "blocking": 0, "advisory": 0,
                                                "reviewed": 1, "allowlisted": 0})
            self.assertEqual(len(section["entries"]), 1)
            self.assertEqual(section["entries"][0]["finding_id"], receipt["findings"][0]["finding_id"])
            captured = copy.deepcopy(section)
            receipt["package_evidence"]["component"] = "runtime-omarchy"
            with self.assertRaises(gate.ScanRefusal):
                gate.validate_receipt(receipt, actual, "credential", None, component="runtime",
                                      package_section=captured)

            with self.assertRaises(gate.ScanRefusal):
                gate.verified_staging_scan(archive, self.root / "no-approved-evidence", component="runtime")

            from image_scanner_policy import load_content_policy

            altered = self.root / "policy-copy" / ".github" / "scripts"
            protected = json.loads((scripts / "image-content-manifest.json").read_bytes())["files"]
            for name in ["image-content-manifest.json", *protected]:
                destination = altered / name
                destination.parent.mkdir(parents=True, exist_ok=True)
                destination.write_bytes((scripts / name).read_bytes())
            self.assertIsNotNone(load_content_policy(altered.parents[1]))
            validator = altered / "image_content_policy.py"
            validator.write_bytes(validator.read_bytes() + b"\n# altered fixture\n")
            with patch.object(gate, "load_package_evidence", return_value=evidence), \
                    patch.object(gate, "load_content_policy", side_effect=lambda _: load_content_policy(altered.parents[1])), \
                    self.assertRaisesRegex(gate.ScanRefusal, "Package evidence refused"):
                gate.verified_staging_scan(archive, self.root / "altered-validator", component="runtime")

    def proof_command(self, name, payload, locator):
        archive = self.root / name
        archive.write_bytes(payload)
        output = self.root / (name + ".proof.json")
        cmd = [sys.executable, "-B", str(Path(__file__).resolve().parents[1] / "prove_package_member.py"),
               "--artifact", str(archive), "--locator", locator, "--output", str(output),
               "--authenticated-sha256"]
        return cmd, output

    def test_member_proof_cli_checks_authenticated_artifact_before_writing(self):
        payload = tar_bytes([(SOURCE, PUBLIC_SOURCE)])
        cmd, output = self.proof_command("vendor-package", payload, "0")
        wrong = subprocess.run([*cmd, "f" * 64], capture_output=True, check=False, timeout=30)
        self.assertEqual(wrong.returncode, 2)
        self.assertFalse(output.exists())
        valid = subprocess.run([*cmd, digest_bytes(payload)], capture_output=True, check=False, timeout=30)
        self.assertEqual(valid.returncode, 0)
        self.assertNotIn(SOURCE.encode(), valid.stdout + valid.stderr)
        proof = json.loads(output.read_bytes())
        self.assertEqual(proof["artifact_sha256"], digest_bytes(payload))
        self.assertEqual(proof["member"], {"locator": "0", "path": [SOURCE],
                                          "type": "regular", "sha256": digest_bytes(PUBLIC_SOURCE)})

    def test_member_proof_cli_reads_debian_and_arch_payloads(self):
        inner = tar_bytes([(SOURCE, PUBLIC_SOURCE)])
        control = gzip.compress(tar_bytes([("control", b"Package: test-only\n")]))
        debian = ar_bytes([("debian-binary/", b"2.0\n"), ("control.tar.gz/", control),
                           ("data.tar.gz/", gzip.compress(inner))])
        arch = subprocess.run(["zstd", "-q", "-c"], input=inner, capture_output=True,
                              check=True, timeout=30).stdout
        for name, payload, locator, path in (
            ("test.deb", debian, "2/0/0", ["data.tar.gz", "decoded", SOURCE]),
            ("test.pkg.tar.zst", arch, "0/0", ["decoded", SOURCE]),
        ):
            with self.subTest(name=name):
                cmd, output = self.proof_command(name, payload, locator)
                result = subprocess.run([*cmd, digest_bytes(payload)], capture_output=True, check=False, timeout=30)
                self.assertEqual(result.returncode, 0, result.stderr)
                proof = json.loads(output.read_bytes())
                self.assertEqual(proof["artifact_sha256"], digest_bytes(payload))
                self.assertEqual(proof["member"], {"locator": locator, "path": path, "type": "regular",
                                                   "sha256": digest_bytes(PUBLIC_SOURCE)})

    def test_zip_nonregular_payload_scans_but_cannot_supply_member_proof(self):
        cases = [(stat.S_IFREG, b"", 0), (stat.S_IFLNK, b"", 2),
                 (stat.S_IFREG, bytes.fromhex("6e750000"), 2)]
        for index, (mode, extra, expected) in enumerate(cases):
            with self.subTest(mode=mode, extra=extra):
                buffer = io.BytesIO()
                entry = zipfile.ZipInfo("source.js")
                entry.create_system = 3
                entry.external_attr = (mode | 0o644) << 16
                entry.extra = extra
                with zipfile.ZipFile(buffer, "w") as wrapped:
                    wrapped.writestr(entry, PUBLIC_SOURCE)
                nested = buffer.getvalue()
                cmd, output = self.proof_command(f"test-{index}.zip", nested, "0")
                result = subprocess.run([*cmd, digest_bytes(nested)], capture_output=True, check=False, timeout=30)
                self.assertEqual(result.returncode, expected)
                self.assertEqual(output.exists(), expected == 0)

                outer = "usr/lib/example/code.zip"
                layer = tar_bytes([(outer, nested)])
                member = {"locator": "0", "path": ["source.js"], "sha256": digest_bytes(PUBLIC_SOURCE)}
                archive = self.root / f"image-{index}.tar"
                docker_archive(archive, [layer])
                raw = self.detector_scan(archive)
                result, final = self.classify(archive, raw, self.policy(self.manifest(
                    layer, layer, file=outer, member=member
                )))
                self.assertEqual(len(result["entries"]), int(expected == 0))
                self.assertEqual(len(final["blocking_findings"]), int(expected != 0))
