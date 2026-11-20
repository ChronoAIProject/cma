"""Check normalized native evidence through cold and warm publication receipts."""

import copy
import os
import shutil
import subprocess
import sys
import tarfile
import unittest
from unittest.mock import patch

from image_secrets import derived_payload, key_marker_view, native_json_key_view, package_receipt
from image_secrets.common import digest_bytes
from image_secrets.gate import FORMAT_MARKER_ADVISORY, KEY_MARKER_VIEW_ADVISORY, KEY_MARKER_VIEW_RULE
from image_secrets.package_evidence import OMARCHY_PORTABLE_SCHEMA
from tests import fixtures
from tests import test_derived_payload as derived_tests
from tests import test_key_marker_view as key_tests
from tests import test_package_policy as packages

FILE = key_marker_view.PATHS["file"]
PREFIX = b'published fixture \xc2\xa9\xff\n<begin value="'
MIDDLE = b'"/>\n  <public record="\xff format label"/>\n  <end value="'
SUFFIX = b'"/>\n'

NATIVE_TEST_DETECTOR = r'''
import json
import subprocess
import sys
import tempfile
from pathlib import Path

binary, action, *arguments = sys.argv[1:]
if action == "version":
    raise SystemExit(subprocess.call([binary, action, *arguments]))

stage = Path(arguments[0])
with tempfile.TemporaryDirectory(prefix="detector-fixture-") as temporary:
    shadow = Path(temporary)
    for original in stage.iterdir():
        data = original.read_bytes()
        if data.startswith(b"published fixture "):
            data = data.decode("utf-8", "replace").encode("utf-8")
        (shadow / original.name).write_bytes(data)
    result = subprocess.run([binary, action, str(shadow), *arguments[1:]], check=False)
    if result.returncode in (0, 1):
        report = Path(arguments[arguments.index("--report-path") + 1])
        findings = json.loads(report.read_bytes())
        for row in findings:
            row["File"] = str(stage / Path(row["File"]).name)
        report.write_text(json.dumps(findings))
    raise SystemExit(result.returncode)
'''


class NativeJSONKeyViewTests(unittest.TestCase):
    tearDown = packages.PackageTests.tearDown
    scan = packages.PackageTests.scan
    detector_scan = packages.PackageTests.detector_scan
    manifest = packages.PackageTests.manifest
    refused = key_tests.KeyMarkerViewTests.refused
    publish = derived_tests.DerivedPayloadTests.publish
    binding = derived_tests.DerivedPayloadTests.binding

    def setUp(self):
        packages.PackageTests.setUp(self)
        detector = shutil.which("gitleaks")
        self.assertIsNotNone(detector)
        binary_directory = self.root / "test-detector"
        binary_directory.mkdir()
        wrapper = binary_directory / "gitleaks"
        # Gitleaks builds differ in their treatment of ill-formed UTF-8.
        # Supply the rendered view to the real detector in this fixture only.
        # The scanner still checks and reopens the original archive bytes.
        wrapper.write_text(f"#!{sys.executable}\nimport sys\nsys.argv.insert(1, {detector!r})\n"
                           + NATIVE_TEST_DETECTOR)
        wrapper.chmod(0o700)
        self.original_path = os.environ["PATH"]
        environment = patch.dict(os.environ, {"PATH": str(binary_directory) + os.pathsep + self.original_path})
        environment.start()
        self.addCleanup(environment.stop)

    def archive_fixture(self, contents, path=FILE):
        layer = fixtures.tar_bytes([(path, contents), ("etc/machine-id", b""),
                                    ("var/lib/dbus/machine-id", (tarfile.SYMTYPE, "/etc/machine-id"))])
        archive = self.root / "native-records.tar"
        fixtures.docker_archive(archive, [layer])
        return archive, layer

    def fixture(self, *, suffix=SUFFIX, middle=MIDDLE, prefix=PREFIX, path=FILE):
        chunks = (key_tests.BEGIN, middle, key_tests.END)
        original = b"".join(chunks)
        normalized = native_json_key_view.render(original)
        contents = prefix + original + suffix
        archive, layer = self.archive_fixture(contents, path)
        report = self.detector_scan(archive, key_marker_view.SCOPE)
        [image] = report["inventory"]["images"]
        candidates = [row for row in report["blocking_findings"] if row["rule"] == KEY_MARKER_VIEW_RULE
                      and row["candidate_sha256"] == digest_bytes(normalized)]
        self.assertTrue(candidates, "the fixture detector must report the exact rendered marker")
        finding = min(candidates, key=lambda row: row["line"])
        generated = path != FILE
        source_path = derived_payload.MIME_SOURCE if generated else path
        source = b"<mime-test-catalog/>\n" if generated else contents
        evidence = self.manifest(fixtures.tar_bytes([(source_path, source)]), layer, file=source_path,
                                 component=key_marker_view.SCOPE)
        evidence["packages"][0].update(name="shared-mime-info" if generated else "file",
                                       authority_type="arch-signature", architecture="x86_64")
        [entry] = evidence["entries"]
        entry["path"] = path
        if generated:
            self.binding(entry, report, finding, "mime-index", contents)
        proof = {"finding_id": finding["finding_id"],
                 "original": {"offset": len(prefix), "size": len(original), "sha256": digest_bytes(original)},
                 "detector": {"offset": len(native_json_key_view.render(prefix)), "size": len(normalized),
                              "sha256": digest_bytes(normalized)},
                 "parts": [{"kind": kind, "size": len(chunk), "sha256": digest_bytes(chunk),
                            "source": "fixtures/magic", "line": line, "proof_sha256": digest_bytes(chunk)}
                           for line, (kind, chunk) in enumerate(zip(("begin", "records", "end"), chunks), 1)]}
        entry["native_json_key_view"] = {"encoding": native_json_key_view.ENCODING,
                                         "archive_sha256": report["archive_sha256"], "image_config": image["config_digest"],
                                         "image_manifest": image["manifest"], "matches": [proof]}
        entry["matches"] = [{"rule": finding["rule"], "line": finding["line"],
                             "candidate_sha256": finding["candidate_sha256"],
                             "explanation": KEY_MARKER_VIEW_ADVISORY, "proof_sha256": "4" * 64}]
        return archive, evidence, report, proof

    def test_unwrapped_detector_retains_independent_private_material(self):
        key = subprocess.run(["openssl", "genpkey", "-algorithm", "EC", "-pkeyopt", "ec_paramgen_curve:prime256v1",
                              "-outform", "DER"], capture_output=True, check=True, timeout=10).stdout
        archive, _ = self.archive_fixture(PREFIX + key_tests.BEGIN + MIDDLE + key_tests.END + SUFFIX + key)
        with patch.dict(os.environ, {"PATH": self.original_path}):
            raw = self.detector_scan(archive, key_marker_view.SCOPE)
        self.assertTrue(raw["complete"])
        self.assertEqual(raw["allowlisted"], [])
        self.assertGreaterEqual({row["rule"] for row in raw["blocking_findings"]},
                                {"content.private-key-header", "content.binary-private-key"})

    def test_exact_view_reconstructs_identical_cold_and_warm_receipts(self):
        archive, evidence, raw, proof = self.fixture()
        cache = self.root / "native-cache.json"
        options = ["--detector-cache-context", "a" * 64, "--detector-cache-output", str(cache)]
        cold = self.refused(archive, evidence, "cold-native", options=options)
        warm = self.refused(archive, evidence, "warm-native", options=[*options, "--detector-cache-input", str(cache)])
        self.assertEqual(warm["detector_cache"]["hits"], 1)
        self.assertEqual(cold["package_evidence"], warm["package_evidence"])
        self.assertEqual(cold["findings"], raw["findings"])
        [receipt] = cold["package_evidence"]["entries"]
        self.assertEqual(receipt["native_json_key_view"]["original"], proof["original"])
        self.assertEqual(receipt["native_json_key_view"]["detector"], proof["detector"])
        self.assertEqual([item["rule"] for item in cold["blocking_findings"]], ["content.private-key-header"])

    def complete_catalog(self, manifest, raw, prefix, text, *, view="native_json_key_view"):
        [row] = manifest["entries"]
        [marker] = [item for item in raw["blocking_findings"] if item["rule"] == "content.private-key-header"]
        binding = row[view]
        row["reviewed_literals"] = {key: binding[key] for key in ("archive_sha256", "image_config", "image_manifest")}
        row["reviewed_literals"]["findings"] = [{
            "rule": marker["rule"], "finding_id": marker["finding_id"],
            "spans": [{"offset": len(prefix), "size": len(key_tests.BEGIN), "sha256": digest_bytes(key_tests.BEGIN),
                       "context": {"offset": 0, "size": len(text), "sha256": digest_bytes(text)},
                       "source": {"path": "fixtures/magic", "line": 1, "sha256": digest_bytes(text)}}],
        }]
        row["matches"].append({"rule": marker["rule"], "candidate_sha256": None, "line": None,
                               "explanation": FORMAT_MARKER_ADVISORY, "proof_sha256": digest_bytes(text)})
        if "derived_payload" in row:
            row["derived_payload"]["findings"].append(marker["finding_id"])

    def test_final_gate_verifies_native_view_when_every_finding_is_resolved(self):
        archive, manifest, raw, _ = self.fixture()
        text = PREFIX + key_tests.BEGIN + MIDDLE + key_tests.END + SUFFIX
        self.complete_catalog(manifest, raw, PREFIX, text)
        verified = self.publish(archive, manifest, "native-publish")
        self.assertEqual(verified["findings"], raw["findings"])
        self.assertEqual(verified["blocking_findings"], [])
        self.assertEqual({row["classification"] for row in verified["package_evidence"]["entries"]},
                         {KEY_MARKER_VIEW_ADVISORY, FORMAT_MARKER_ADVISORY})
        section = verified["package_evidence"]
        [checked] = [row for row in section["entries"] if "native_json_key_view" in row]
        checked["key_marker_view"] = copy.deepcopy(checked["native_json_key_view"])
        verified["policy"]["package_evidence_sha256"] = package_receipt.digest(section)
        with patch.object(sys, "path", [str(key_tests.SCRIPTS), *sys.path]):
            import image_secret_gate as gate

            with self.assertRaises(gate.ScanRefusal):
                gate.validate_receipt(
                    verified, verified["archive_sha256"], "credential", None,
                    component=key_marker_view.SCOPE, package_section=section)

    def test_final_gate_verifies_generated_mime_input_and_output_separately(self):
        path = "usr/share/mime/magic"
        suffix = b'"/>\n'
        label = b'\xff<end="' + b"." * 100
        size = len(label) + len(key_tests.END) + len(suffix) - 1
        primary = derived_tests.mime_magic(b"application/pkcs8", key_tests.BEGIN)
        prefix = primary[:-len(key_tests.BEGIN) - 1]
        middle = b"\n>0=" + size.to_bytes(2, "big") + label
        text = prefix + key_tests.BEGIN + middle + key_tests.END + suffix
        archive, manifest, raw, _ = self.fixture(path=path, prefix=prefix, middle=middle, suffix=suffix)
        self.complete_catalog(manifest, raw, prefix, text)
        verified = self.publish(archive, manifest, "generated-publish")
        [result] = [row for row in verified["package_evidence"]["entries"]
                    if row["classification"] == KEY_MARKER_VIEW_ADVISORY]
        self.assertNotEqual(result["package_member"]["sha256"], result["outer_file_sha256"])
        self.assertEqual(result["package_member"]["sha256"], result["derived_payload"]["input_sha256"])
        self.assertEqual(result["outer_file_sha256"], result["derived_payload"]["output_sha256"])
        for field in ("input_sha256", "output_sha256"):
            changed = copy.deepcopy(manifest)
            [row] = changed["entries"]
            row["derived_payload"][field] = digest_bytes(field.encode())
            self.assertIsNone(self.refused(archive, changed, "changed-" + field, classification="content_refused"))

    def test_portable_native_markers_use_current_pacman_owner_and_layer(self):
        cases = (
            ("key_marker_view", key_tests.PREFIX, key_tests.RECORDS, key_tests.SUFFIX,
             key_tests.KeyMarkerViewTests.fixture),
            ("native_json_key_view", PREFIX, MIDDLE, SUFFIX, NativeJSONKeyViewTests.fixture),
        )
        for view, prefix, middle, suffix, fixture in cases:
            with self.subTest(view=view):
                archive, catalog, raw, _ = fixture(self)
                contents = prefix + key_tests.BEGIN + middle + key_tests.END + suffix
                self.complete_catalog(catalog, raw, prefix, contents, view=view)
                [row] = catalog["entries"]
                [package] = catalog["packages"]
                name, version = package["name"], package["version"]
                path = row["path"]
                mtree = f"#mtree\n./{path} type=file sha256digest={digest_bytes(contents)}\n".encode()
                catalog["schema"] = OMARCHY_PORTABLE_SCHEMA
                row.update(occurrences=1, ownership={"mtree_sha256": digest_bytes(mtree)})
                for field in ("layer", "diff_id", "entry"):
                    del row[field]
                for field in (view, "reviewed_literals"):
                    proof = row[field]
                    for key in ("archive_sha256", "image_config", "image_manifest"):
                        del proof[key]
                    for entry in proof["findings" if field == "reviewed_literals" else "matches"]:
                        del entry["finding_id"]

                def repack(installed_version):
                    root = "var/lib/pacman/local"
                    label = f"{root}/{name}-{installed_version}"
                    desc = f"%NAME%\n{name}\n\n%VERSION%\n{installed_version}\n\n%ARCH%\nx86_64\n".encode()
                    entries = [(f"{root}/ALPM_DB_VERSION", b"9\n"), (f"{label}/desc", desc),
                               (f"{label}/files", f"%FILES%\n{path}\n".encode()), (f"{label}/mtree", mtree),
                               ("etc/machine-id", b""), ("var/lib/dbus/machine-id", (tarfile.SYMTYPE, "/etc/machine-id")),
                               (path, contents)]
                    fixtures.oci_archive(archive, [fixtures.tar_bytes([("padding", b"ordinary")]),
                                                   fixtures.tar_bytes(entries)])

                repack(version)
                result = self.publish(archive, catalog, view)
                self.assertNotEqual(result["archive_sha256"], raw["archive_sha256"])
                [receipt] = [entry for entry in result["package_evidence"]["entries"] if view in entry]
                self.assertEqual(receipt["layer"], 1)
                self.assertEqual(receipt[view]["archive_sha256"], result["archive_sha256"])
                self.assertEqual(receipt[view]["finding_id"], receipt["finding_id"])
                repack("2.0" if version != "2.0" else "3.0")
                denied = self.refused(archive, catalog, view + "-different-owner")
                self.assertEqual(denied["package_evidence"]["entries"], [])
                self.assertEqual(len(denied["blocking_findings"]), 2)

    def test_changed_or_uncovered_ranges_keep_each_raw_finding(self):
        archive, evidence, raw, _ = self.fixture()
        for name, change in (("original", "offset"), ("detector", "offset"), ("detector", "size")):
            with self.subTest(view=name, field=change):
                changed = copy.deepcopy(evidence)
                changed["entries"][0]["native_json_key_view"]["matches"][0][name][change] += 1
                report = self.refused(archive, changed, f"changed-{name}-{change}")
                self.assertEqual(report["blocking_findings"], raw["blocking_findings"])
        claim = evidence["entries"][0]["native_json_key_view"]["matches"][0]
        claim["parts"][-1]["size"] += 1
        claim["parts"][1]["size"] -= 1
        report = self.refused(archive, evidence, "uncovered-record-byte")
        self.assertEqual(report["package_evidence"]["entries"], [])
        self.assertEqual(report["blocking_findings"], raw["blocking_findings"])

    def test_literal_replacement_character_does_not_supply_a_unique_view(self):
        second = native_json_key_view.render(key_tests.BEGIN + MIDDLE + key_tests.END)
        archive, evidence, raw, _ = self.fixture(suffix=SUFFIX + second + SUFFIX)
        report = self.refused(archive, evidence, "ambiguous-native-view")
        self.assertEqual(report["package_evidence"]["entries"], [])
        self.assertEqual(report["blocking_findings"], raw["blocking_findings"])

    def test_independently_detected_binary_key_prevents_native_advisory(self):
        key = subprocess.run(["openssl", "genpkey", "-algorithm", "EC", "-pkeyopt", "ec_paramgen_curve:prime256v1",
                              "-outform", "DER"], capture_output=True, check=True, timeout=10).stdout
        archive, evidence, raw, _ = self.fixture(suffix=SUFFIX + key)
        report = self.refused(archive, evidence, "independent-binary-key")
        self.assertEqual(report["package_evidence"]["entries"], [])
        self.assertEqual(report["blocking_findings"], raw["blocking_findings"])
        self.assertIn("content.binary-private-key", {row["rule"] for row in report["blocking_findings"]})

    def test_incomplete_render_has_no_receipt(self):
        archive, evidence, raw, _ = self.fixture()
        with patch.object(native_json_key_view, "MAX_RENDER_SECONDS", 0):
            report = self.refused(archive, evidence, "expired-render-budget")
        self.assertEqual(report["package_evidence"]["entries"], [])
        self.assertEqual(report["blocking_findings"], raw["blocking_findings"])

    def test_rendered_candidate_limit_applies_after_invalid_byte_expansion(self):
        base = native_json_key_view.render(key_tests.BEGIN + MIDDLE + key_tests.END)
        for size in (32768, 32769):
            with self.subTest(size=size):
                middle = MIDDLE + b"." * (size - len(base))
                archive, evidence, raw, proof = self.fixture(middle=middle)
                original = PREFIX + key_tests.BEGIN + middle + key_tests.END + SUFFIX
                self.assertEqual(proof["detector"]["size"], size)
                valid = size == native_json_key_view.MAX_MATCH_BYTES
                self.assertEqual(native_json_key_view.verified_view(
                    original, native_json_key_view.render(original), b"", b"", proof), valid)
                if valid:
                    report = self.refused(archive, evidence, "exact-rendered-budget")
                    self.assertEqual(report["findings"], raw["findings"])
                    self.assertEqual(len(report["package_evidence"]["entries"]), 1)
                else:
                    self.assertIsNone(self.refused(archive, evidence, "over-rendered-budget",
                                                   classification="content_refused"))
