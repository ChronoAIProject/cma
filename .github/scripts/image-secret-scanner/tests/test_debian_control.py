"""Exercise publication receipts for path detections on native Debian controls."""

import copy
import gzip
import json
import sys
import tarfile
import unittest
from pathlib import Path
from unittest.mock import patch

from image_secrets.common import digest_bytes, tar_metadata
from image_secrets.gate import DEBIAN_CONTROL_ADVISORY, DEBIAN_CONTROL_RULE
from image_secrets.package_evidence import PORTABLE_SCHEMA
from image_secrets.package_occurrences import prove_package_member
from image_secrets.portable_ownership import STATUS_PATH
from tests import fixtures
from tests import test_package_policy as packages
from tests.test_runtime_archives import ar_bytes

SCRIPTS = Path(__file__).resolve().parents[2]
NAME = "libfixtureapi"
CONTROL_PATH = f"var/lib/dpkg/info/{NAME}:amd64.triggers"
CANDIDATE = b"amd64.triggers"
CONTROL = b"interest-noawait ldconfig\n"


class DebianControlTests(unittest.TestCase):
    setUp = packages.PackageTests.setUp
    tearDown = packages.PackageTests.tearDown
    scan = packages.PackageTests.scan
    detector_scan = packages.PackageTests.detector_scan

    def fixture(self, *, payload=CONTROL, signed_payload=None, attributes=None):
        declared = payload if signed_payload is None else signed_payload
        vendor = self.root / "vendor.deb"
        vendor.write_bytes(ar_bytes([
            ("debian-binary", b"2.0\n"),
            ("control.tar.gz", gzip.compress(fixtures.tar_bytes([("triggers", declared)]), mtime=0)),
        ]))
        package_sha = digest_bytes(vendor.read_bytes())
        member = prove_package_member(vendor, package_sha, "1/0/0", self.root)
        attrs = {CONTROL_PATH: attributes} if attributes else None
        layer = fixtures.tar_bytes([
            (CONTROL_PATH, payload), ("etc/machine-id", b""),
            ("var/lib/dbus/machine-id", (tarfile.SYMTYPE, "/etc/machine-id")),
        ], pax_headers=attrs)
        archive = self.root / "control-image.tar"
        fixtures.docker_archive(archive, [layer])
        report = self.detector_scan(archive)
        candidate_digest = digest_bytes(CANDIDATE)
        [finding] = [item for item in report["blocking_findings"] if item["rule"] == DEBIAN_CONTROL_RULE
                     and item.get("candidate_sha256") == candidate_digest]
        [image] = report["inventory"]["images"]
        view_info = tarfile.TarInfo(CONTROL_PATH)
        if attributes:
            view_info.pax_headers.update(attributes)
        view = tar_metadata(CONTROL_PATH, view_info)
        self.assertEqual(digest_bytes(view), finding["file_sha256"])
        proof = {"archive_sha256": report["archive_sha256"], "image_config": image["config_digest"],
                 "image_manifest": image["manifest"], "finding_id": finding["finding_id"],
                 "view_sha256": digest_bytes(view), "span": {"offset": view.index(CANDIDATE),
                                                             "size": len(CANDIDATE), "sha256": candidate_digest}}
        manifest = {"schema": "cma-staging-package-evidence-v1", "scanner_manifest_sha256": packages.SCANNER,
                    "packages": [{"name": NAME, "version": "1", "architecture": "amd64",
                                  "artifact_sha256": package_sha, "authority_type": "debian-index",
                                  "authority_sha256": "3" * 64}],
                    "entries": [{"component": "runtime", "platform": "linux/amd64", "layer": 0,
                                 "diff_id": "sha256:" + digest_bytes(layer), "entry": 0, "path": CONTROL_PATH,
                                 "file_sha256": digest_bytes(declared), "package_sha256": package_sha,
                                 "package_member": member, "member": None, "debian_control": proof,
                                 "matches": [{"rule": DEBIAN_CONTROL_RULE, "candidate_sha256": candidate_digest,
                                              "line": 1, "explanation": DEBIAN_CONTROL_ADVISORY,
                                              "proof_sha256": "4" * 64}]}]}
        return archive, manifest, report

    def publish(self, archive, manifest, name, *, expected_error=None, options=()):
        with patch.object(sys, "path", [str(SCRIPTS), *sys.path]):
            import image_secret_gate as gate

            pinned = copy.deepcopy(manifest)
            pinned["scanner_manifest_sha256"] = gate.MANIFEST_SHA256
            raw = json.dumps(pinned).encode()
            evidence = raw, digest_bytes(raw), packages.REVIEW
            output = self.root / name
            with (patch.object(gate, "load_package_evidence", return_value=evidence),
                  patch.object(gate, "scanner_options", return_value=list(options))):
                if expected_error is None:
                    gate.verified_staging_scan(archive, output, component="runtime")
                    return json.loads((output / "REPORT.json").read_bytes())
                with self.assertRaises(gate.ScanRefusal) as refusal:
                    gate.verified_staging_scan(archive, output, component="runtime")
                self.assertEqual(refusal.exception.outcome, expected_error)
                return getattr(refusal.exception, "package_report", None)

    def test_receipt_separates_authenticated_payload_and_detector_view_on_cache_hits(self):
        archive, manifest, raw = self.fixture()
        cache = self.root / "control-cache.json"
        options = ["--detector-cache-context", "d" * 64, "--detector-cache-output", str(cache)]
        cold = self.publish(archive, manifest, "cold", options=options)
        warm = self.publish(archive, manifest, "warm", options=[*options, "--detector-cache-input", str(cache)])
        self.assertEqual(warm["detector_cache"]["hits"], 1)
        self.assertEqual(warm["package_evidence"], cold["package_evidence"])
        self.assertEqual(cold["findings"], raw["findings"])
        [row] = cold["package_evidence"]["entries"]
        self.assertEqual(row["classification"], DEBIAN_CONTROL_ADVISORY)
        self.assertEqual(row["file_sha256"], raw["blocking_findings"][0]["file_sha256"])
        self.assertEqual(row["outer_file_sha256"], digest_bytes(CONTROL))
        self.assertNotEqual(row["file_sha256"], row["outer_file_sha256"])

    def test_same_member_with_uncovered_payload_remains_blocking(self):
        archive, manifest, raw = self.fixture(payload=CONTROL + packages.PUBLIC_SOURCE)
        result = self.publish(archive, manifest, "mixed", expected_error="scanner_findings")
        self.assertEqual(result["findings"], raw["findings"])
        self.assertEqual([row["rule"] for row in result["blocking_findings"]], [packages.RULE])
        self.assertEqual([row["rule"] for row in result["package_evidence"]["entries"]], [DEBIAN_CONTROL_RULE])

    def test_replay_refuses_changed_payload_unproved_view_and_duplicate_pax_candidate(self):
        for name, inputs in (("changed-payload", {"payload": CONTROL + b"\n", "signed_payload": CONTROL}),
                             ("wrong-view", {}), ("duplicate-pax", {"attributes": {"comment": CANDIDATE.decode()}})):
            with self.subTest(case=name):
                archive, manifest, raw = self.fixture(**inputs)
                if name == "wrong-view":
                    manifest["entries"][0]["debian_control"]["view_sha256"] = "f" * 64
                result = self.publish(archive, manifest, name, expected_error="scanner_findings")
                self.assertEqual(result["package_evidence"]["entries"], [])
                self.assertEqual(result["blocking_findings"], raw["blocking_findings"])
                archive.unlink()

    def test_installed_path_must_be_the_native_package_projection(self):
        archive, manifest, _ = self.fixture()
        manifest["packages"][0]["architecture"] = "arm64"
        self.assertIsNone(self.publish(archive, manifest, "wrong-architecture", expected_error="content_refused"))

    def test_portable_trigger_path_is_derived_and_digest_checked(self):
        archive, manifest, _ = self.fixture()
        status = (f"Package: {NAME}\nVersion: 1\nArchitecture: amd64\nStatus: install ok installed\n").encode()
        layer = fixtures.tar_bytes([
            (CONTROL_PATH, CONTROL), (STATUS_PATH, status), ("etc/machine-id", b""),
            ("var/lib/dbus/machine-id", (tarfile.SYMTYPE, "/etc/machine-id")),
        ])
        fixtures.oci_archive(archive, [layer])
        manifest["schema"] = PORTABLE_SCHEMA
        [row] = manifest["entries"]
        for key in ("layer", "diff_id", "entry"):
            del row[key]
        for key in ("archive_sha256", "image_config", "image_manifest", "finding_id"):
            del row["debian_control"][key]
        row.update(path={"multiarch": True, "sha256": digest_bytes(CONTROL_PATH.encode())},
                   occurrences=1, ownership={})
        recipe = self.publish(archive, manifest, "portable-trigger")
        [evidence] = recipe["package_evidence"]["entries"]
        self.assertEqual(evidence["debian_control"]["installed_path"], CONTROL_PATH)
        self.assertEqual(evidence["current_owner"]["dpkg_status"]["entry"], 1)
        row["path"]["multiarch"] = False
        self.assertIsNone(self.publish(archive, manifest, "wrong-path-digest", expected_error="content_refused"))
