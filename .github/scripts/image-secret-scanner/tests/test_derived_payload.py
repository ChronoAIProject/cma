"""Reconstruct reviewed build outputs at the staging publication boundary."""

import base64
import copy
import csv
import hashlib
import io
import json
import stat
import struct
import sys
import tarfile
import unittest
import zipfile
from pathlib import Path
from unittest.mock import patch

from image_secrets import derived_payload, mime_markers, nested_certificates, reviewed_literals
from image_secrets.common import digest_bytes
from tests import fixtures
from tests import test_executable_vectors as executable
from tests import test_package_policy as packages

SCRIPTS = Path(__file__).resolve().parents[2]
OPENSSL = "usr/bin/openssl"
CONSTANT = hashlib.sha256(b"synthetic published build constant").hexdigest().encode()
MARKER = b"-----BEGIN PGP " + b"PRIVATE KEY BLOCK-----"
RECORD = "pip-26.2.1.dist-info/RECORD"


def packed_wheel(entries, *, recorded=None):
    manifest = io.StringIO(newline="")
    writer = csv.writer(manifest)
    for name, data in entries if recorded is None else recorded:
        digest = base64.urlsafe_b64encode(hashlib.sha256(data).digest()).decode("ascii").rstrip("=")
        writer.writerow((name, "sha256=" + digest, len(data)))
    writer.writerow((RECORD, "", ""))
    record = manifest.getvalue().encode()
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w") as archive:
        for name, body in [*entries, (RECORD, record)]:
            member = zipfile.ZipInfo(name)
            member.create_system = 3
            member.external_attr = (stat.S_IFREG | 0o644) << 16
            member.compress_type = zipfile.ZIP_DEFLATED
            archive.writestr(member, body)
    return output.getvalue(), digest_bytes(record)


def mime_cache(mime_type, marker=MARKER):
    indexes = struct.pack(">III", 1, len(marker), 52)
    match = struct.pack(">4I", 50, 100, 1, 68)
    discriminator = struct.pack(">8I", 0, 1, 1, len(marker), 100 + len(mime_type) + 1, 0, 0, 0)
    header = b"\0\x01\0\x02" + b"\0" * 20 + struct.pack(">I", 40) + b"\0" * 12
    return header + indexes + match + discriminator + mime_type + b"\0" + marker + b"\0"


def mime_magic(mime_type, marker=MARKER):
    return (mime_markers.MAGIC_HEADER + b"[50:" + mime_type + b"]\n>0="
            + len(marker).to_bytes(2, "big") + marker + b"\n")


class DerivedPayloadTests(unittest.TestCase):
    setUp = packages.PackageTests.setUp
    tearDown = packages.PackageTests.tearDown
    scan = packages.PackageTests.scan
    detector_scan = packages.PackageTests.detector_scan
    manifest = packages.PackageTests.manifest

    def fixture(self, name, path, source, data, *, component="runtime", copies=1):
        layers = [fixtures.tar_bytes([(path, data), ("etc/machine-id", b""),
                                     ("var/lib/dbus/machine-id", (tarfile.SYMTYPE, "/etc/machine-id")),
                                     (f"build-marker-{index}", b"")], modes={path: 0o755})
                  for index in range(copies)]
        archive = self.root / (name + ".tar")
        fixtures.docker_archive(archive, layers)
        report = self.detector_scan(archive, component)
        manifest = self.manifest(fixtures.tar_bytes([source]), layers[0], file=source[0], component=component)
        return archive, manifest, report, layers

    def binding(self, row, report, finding, kind, data):
        [image] = report["inventory"]["images"]
        row["file_sha256"] = digest_bytes(data)
        row["derived_payload"] = {
            "kind": kind, "archive_sha256": report["archive_sha256"], "image_config": image["config_digest"],
            "image_manifest": image["manifest"], "input_sha256": row["package_member"]["sha256"],
            "output_sha256": row["file_sha256"], "findings": [finding["finding_id"]],
            "producer": {key + "_sha256": digest_bytes(key.encode())
                         for key in ("attestation", "identity", "recipe", "toolchain", "inputs")},
        }
        return row["derived_payload"]

    def compiled(self, name, *, copies=1, extra=b""):
        prefix = executable.ELF + b"\0\nFIPSKEY="
        contents = prefix + CONSTANT + b"\0\n" + extra
        archive, manifest, report, layers = self.fixture(name, OPENSSL, ("Configure", b"fixture build recipe"),
                                                         contents, copies=copies)
        manifest["packages"][0]["name"] = "openssl"
        [base] = manifest["entries"]
        manifest["entries"] = []
        for index, layer in enumerate(layers):
            [finding] = [row for row in report["blocking_findings"]
                         if row["rule"] == derived_payload.COMPILED_RULE and row["layer"] == index]
            row = dict(copy.deepcopy(base), path=OPENSSL, layer=index, diff_id="sha256:" + digest_bytes(layer))
            row["matches"] = [{
                "rule": finding["rule"], "candidate_sha256": finding["candidate_sha256"], "line": finding.get("line"),
                "explanation": "public-noncredential-constant", "proof_sha256": digest_bytes(b"reviewed source line"),
            }]
            proof = self.binding(row, report, finding, "compiled-executable", contents)
            proof["span"] = {"offset": len(prefix), "size": len(CONSTANT), "sha256": digest_bytes(CONSTANT)}
            manifest["entries"].append(row)
        return archive, manifest, report

    def wheel(self, name, *, extra=b"", stale_record=False):
        src = b"src = 1\n"
        wheel_path = nested_certificates.WHEEL
        inside = "fixture/source.py"
        entries = [(inside, packages.PUBLIC_SOURCE + extra)]
        wheel, record = packed_wheel(entries, recorded=[(inside, src)] if stale_record else None)
        vendor, _ = packed_wheel([(inside, src)])
        archive, manifest, report, _ = self.fixture(name, wheel_path, (wheel_path, vendor), wheel,
                                                    component=nested_certificates.COMPONENT)
        manifest["packages"][0].update(name="python", authority_type="arch-signature", architecture="x86_64")
        [row] = manifest["entries"]
        [finding] = [item for item in report["blocking_findings"]
                     if item["rule"] == packages.RULE and item.get("member") == "0"
                     and item["line"] == 1 and item["candidate_sha256"] == digest_bytes(packages.SYMBOL)]
        row["member"] = {"locator": "0", "path": [inside], "sha256": digest_bytes(entries[0][1])}
        self.binding(row, report, finding, "repacked-wheel", wheel)["record_sha256"] = record
        return archive, manifest, report

    def publish(self, archive, manifest, name, *, blocks=False, options=(), source=None):
        with patch.object(sys, "path", [str(SCRIPTS), *sys.path]):
            import image_secret_gate as gate

            component = manifest["entries"][0]["component"]
            raw = json.dumps(dict(manifest, scanner_manifest_sha256=gate.MANIFEST_SHA256)).encode()
            output = self.root / name
            with (patch.object(gate, "load_package_evidence", return_value=(raw, digest_bytes(raw), packages.REVIEW)),
                  patch.object(gate, "scanner_options", return_value=list(options))):
                if not blocks:
                    gate.verified_staging_scan(archive, output, component=component, source=source)
                    return json.loads((output / "REPORT.json").read_bytes())
                with self.assertRaises(gate.ScanRefusal) as refusal:
                    gate.verified_staging_scan(archive, output, component=component, source=source)
                self.assertEqual(refusal.exception.outcome, "scanner_findings")
                return refusal.exception.package_report

    def test_compiled_output_has_distinct_receipts_in_two_layers(self):
        archive, manifest, raw = self.compiled("two-occurrences", copies=2)
        accepted = self.publish(archive, manifest, "compiled")
        self.assertEqual(accepted["findings"], raw["findings"])
        self.assertEqual(accepted["counts"]["blocking"], 0)
        self.assertEqual([row["layer"] for row in accepted["package_evidence"]["entries"]], [0, 1])
        manifest["entries"][1]["derived_payload"]["span"]["offset"] += 1
        refused = self.publish(archive, manifest, "wrong-span", blocks=True)
        self.assertEqual([row["layer"] for row in refused["blocking_findings"]], [1])

    def test_compiled_proof_does_not_cover_an_independent_detector_match(self):
        archive, manifest, raw = self.compiled("mixed", extra=packages.PUBLIC_SOURCE)
        refused = self.publish(archive, manifest, "mixed-compiled", blocks=True)
        self.assertEqual(refused["findings"], raw["findings"])
        self.assertEqual([row["rule"] for row in refused["blocking_findings"]], [packages.RULE])
        self.assertEqual(len(refused["package_evidence"]["entries"]), 1)

    def test_repacked_wheel_requires_complete_record_and_retains_raw_findings(self):
        archive, manifest, raw = self.wheel("wheel")
        cache = self.root / "derived-cache.json"
        options = ["--detector-cache-context", "a" * 64, "--detector-cache-output", str(cache)]
        cold = self.publish(archive, manifest, "wheel-cold", options=options)
        warm = self.publish(archive, manifest, "wheel-warm", options=[*options, "--detector-cache-input", str(cache)])
        self.assertEqual(warm["detector_cache"]["hits"], 1)
        self.assertEqual(cold["findings"], raw["findings"])
        self.assertEqual(warm["package_evidence"], cold["package_evidence"])
        manifest["entries"][0]["derived_payload"]["record_sha256"] = "f" * 64
        refused = self.publish(archive, manifest, "wrong-record", blocks=True)
        self.assertEqual(refused["blocking_findings"], raw["blocking_findings"])

        archive, manifest, raw = self.wheel("stale-record", stale_record=True)
        refused = self.publish(archive, manifest, "unhashed-wheel-change", blocks=True)
        self.assertEqual(refused["blocking_findings"], raw["blocking_findings"])

    def test_repacked_wheel_keeps_a_new_uncovered_payload_finding(self):
        archive, manifest, raw = self.wheel("changed", extra=packages.PUBLIC_SOURCE)
        refused = self.publish(archive, manifest, "mixed-wheel", blocks=True)
        self.assertEqual(refused["findings"], raw["findings"])
        self.assertEqual(len(refused["blocking_findings"]), 1)
        self.assertEqual(len(refused["package_evidence"]["entries"]), 1)

    def mime(self, path, data):
        src = b"<mime-demo/>\n"
        archive, manifest, report, _ = self.fixture("mime", path, (derived_payload.MIME_SOURCE, src), data)
        manifest["packages"][0].update(name="shared-mime-info", authority_type="debian-index", architecture="amd64")
        [row] = manifest["entries"]
        [finding] = report["blocking_findings"]
        row["path"] = path
        row["matches"] = [{"rule": finding["rule"], "candidate_sha256": None, "line": None,
                           "explanation": "published-format-marker", "proof_sha256": digest_bytes(src)}]
        proof = self.binding(row, report, finding, "mime-index", data)
        context = {"offset": 0, "size": len(data), "sha256": digest_bytes(data)}
        source = {"path": derived_payload.MIME_SOURCE, "line": 1, "sha256": digest_bytes(src)}
        spans = [{"offset": start, "size": end - start, "sha256": digest_bytes(data[start:end]),
                  "context": context, "source": source}
                 for start, end, _ in reviewed_literals.occurrences(data, finding["rule"])]
        row["reviewed_literals"] = {key: proof[key] for key in ("archive_sha256", "image_config", "image_manifest")}
        row["reviewed_literals"]["findings"] = [{"rule": finding["rule"], "finding_id": finding["finding_id"],
                                                 "spans": spans}]
        return archive, manifest, report

    def test_mime_output_marker_is_a_complete_public_discriminator(self):
        for name, encode in (("magic", mime_magic), ("mime.cache", mime_cache)):
            with self.subTest(format=name):
                path = "usr/share/mime/" + name
                data = encode(mime_markers.MIME_TYPE)
                archive, manifest, report = self.mime(path, data)
                passed = self.publish(archive, manifest, "generated-" + name)
                self.assertEqual(passed["findings"], report["findings"])
                self.assertEqual(len(passed["package_evidence"]["entries"]), 1)
                manifest["entries"][0]["derived_payload"]["archive_sha256"] = "f" * 64
                refused = self.publish(archive, manifest, "different-image-" + name, blocks=True)
                self.assertEqual(refused["blocking_findings"], report["blocking_findings"])
                archive, manifest, report = self.mime(path, encode(b"application/example"))
                refused = self.publish(archive, manifest, "unrelated-mime-" + name, blocks=True)
                self.assertEqual(refused["blocking_findings"], report["blocking_findings"])

    def test_mime_pem_marker_requires_the_matching_type_and_complete_coverage(self):
        recognized = b"application/pkcs8-encrypted"
        expected = b"-----BEGIN ENCRYPTED " + b"PRIVATE KEY-----"
        for name, encode in (("magic", mime_magic), ("mime.cache", mime_cache)):
            with self.subTest(format=name):
                path = "usr/share/mime/" + name
                archive, manifest, raw = self.mime(path, encode(recognized, expected))
                accepted = self.publish(archive, manifest, "recognized-pem-" + name)
                self.assertEqual(accepted["findings"], raw["findings"])
                self.assertEqual(len(accepted["package_evidence"]["entries"]), 1)

                changed = encode(b"application/pkcs8", expected)
                archive, manifest, raw = self.mime(path, changed)
                refused = self.publish(archive, manifest, "wrong-pem-type-" + name, blocks=True)
                self.assertEqual(refused["blocking_findings"], raw["blocking_findings"])

                extra = (encode(b"application/pkcs8", expected).removeprefix(mime_markers.MAGIC_HEADER)
                         if name == "magic" else expected + b"\0")
                archive, manifest, raw = self.mime(path, encode(recognized, expected) + extra)
                refused = self.publish(archive, manifest, "uncovered-pem-" + name, blocks=True)
                self.assertEqual(refused["blocking_findings"], raw["blocking_findings"])
