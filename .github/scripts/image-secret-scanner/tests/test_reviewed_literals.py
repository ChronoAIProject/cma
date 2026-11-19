"""Exercise complete, independently scoped public literal proofs through publication."""

import base64
import copy
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

from image_secrets import reviewed_literals
from image_secrets.common import digest_bytes
from image_secrets.gate import URL_EXAMPLE_RULE
from tests import fixtures
from tests import test_package_policy as packages

SCRIPTS = Path(__file__).resolve().parents[2]
URL = b"https://" + b"fixture:example@sample.invalid/docs\n"
HEADER = b"-----BEGIN " + b"PRIVATE KEY-----"
MARKER = b'const marker = "' + HEADER + b'";\n'
OPENPGP = b'<match value="' + b"-----BEGIN PGP " + b'PRIVATE KEY BLOCK-----"/>\n'
MARKER_RULE = "content.private-key-header"
PGP_RULE = "content.openpgp-private-key"
SOURCE = "usr/share/fixture/parser-data.txt"
WHEEL = "usr/share/fixture/parser-data.whl"


class LiteralTests(unittest.TestCase):
    setUp = packages.PackageTests.setUp
    tearDown = packages.PackageTests.tearDown
    scan = packages.PackageTests.scan
    detector_scan = packages.PackageTests.detector_scan
    trusted_manifest = packages.PackageTests.manifest

    def fixture(self, contents, selection, *, nested=False, pax=None, mode=stat.S_IFREG,
                compression=zipfile.ZIP_DEFLATED):
        path = WHEEL if nested else SOURCE
        if nested:
            output = io.BytesIO()
            with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED) as zipped:
                for index, payload in enumerate(contents):
                    member = zipfile.ZipInfo(f"data-{index}.txt")
                    member.create_system = 3
                    member.external_attr = (mode | 0o644) << 16
                    member.compress_type = compression
                    zipped.writestr(member, payload)
            payload = output.getvalue()
        else:
            [payload] = contents
        layer = fixtures.tar_bytes([(path, payload), ("etc/machine-id", b""),
                                    ("var/lib/dbus/machine-id", (tarfile.SYMTYPE, "/etc/machine-id"))],
                                   pax_headers={path: pax} if pax else None)
        archive = self.root / "literal.tar"
        fixtures.docker_archive(archive, [layer])
        report = self.detector_scan(archive)
        [image] = report["inventory"]["images"]
        manifest = self.trusted_manifest(fixtures.tar_bytes([(path, payload)]), layer, file=path)
        [template] = manifest["entries"]
        rows = []
        for index, (data, selections) in enumerate(zip(contents, selection)):
            row = copy.deepcopy(template)
            locator = str(index) if nested else None
            if nested:
                row["member"] = {"locator": locator, "path": [f"data-{index}.txt"], "sha256": digest_bytes(data)}
            claims = []
            row["matches"] = []
            for rule, maximum in selections.items():
                [finding] = [f for f in report["blocking_findings"]
                             if f["rule"] == rule and f.get("member") == locator]
                spans = [{"offset": start, "size": end - start, "sha256": digest_bytes(data[start:end]),
                          "context": {"offset": 0, "size": len(data), "sha256": digest_bytes(data)},
                          "source": {"path": "vendor/parser.txt", "line": 1, "sha256": "3" * 64}}
                         for start, end, _ in list(reviewed_literals.occurrences(data, rule))[:maximum]]
                claims.append({"rule": rule, "finding_id": finding["finding_id"], "spans": spans})
                row["matches"].append({"rule": rule, "candidate_sha256": None, "line": None,
                                       "explanation": reviewed_literals.classification(rule), "proof_sha256": "4" * 64})
            row["reviewed_literals"] = {"archive_sha256": report["archive_sha256"], "image_config": image["config_digest"],
                                        "image_manifest": image["manifest"], "findings": claims}
            rows.append(row)
        manifest["entries"] = rows
        return archive, manifest, report

    def publish(self, archive, manifest, name, *, blocks=False, options=()):
        with patch.object(sys, "path", [str(SCRIPTS), *sys.path]):
            import image_secret_gate as gate

            data = json.dumps(dict(manifest, scanner_manifest_sha256=gate.MANIFEST_SHA256)).encode()
            evidence = data, digest_bytes(data), packages.REVIEW
            output = self.root / name
            with (patch.object(gate, "load_package_evidence", return_value=evidence),
                  patch.object(gate, "scanner_options", return_value=list(options))):
                if not blocks:
                    gate.verified_staging_scan(archive, output, component="runtime")
                    return json.loads((output / "REPORT.json").read_bytes())
                with self.assertRaises(gate.ScanRefusal) as refusal:
                    gate.verified_staging_scan(archive, output, component="runtime")
                self.assertEqual(refusal.exception.outcome, "scanner_findings")
                return refusal.exception.package_report

    def test_full_receipts_and_nested_member_positions_survive_warm_scan(self):
        archive, manifest, raw = self.fixture([URL, MARKER + OPENPGP],
                                             [{URL_EXAMPLE_RULE: 1}, {MARKER_RULE: 1, PGP_RULE: 1}], nested=True)
        cache = self.root / "detector-cache.json"
        options = ["--detector-cache-context", "a" * 64, "--detector-cache-output", str(cache)]
        cold = self.publish(archive, manifest, "cold", options=options)
        warm = self.publish(archive, manifest, "warm", options=[*options, "--detector-cache-input", str(cache)])
        self.assertEqual(warm["detector_cache"]["hits"], 1)
        self.assertEqual(warm["package_evidence"], cold["package_evidence"])
        self.assertEqual(warm["findings"], raw["findings"])
        self.assertEqual({(row["member"], row["rule"]) for row in cold["package_evidence"]["entries"]},
                         {("0", URL_EXAMPLE_RULE), ("1", MARKER_RULE), ("1", PGP_RULE)})

    def test_a_second_unreviewed_occurrence_blocks_the_file(self):
        for rule, data in ((URL_EXAMPLE_RULE, URL + URL), (MARKER_RULE, MARKER + MARKER)):
            with self.subTest(rule=rule):
                archive, manifest, raw = self.fixture([data], [{rule: 1}])
                report = self.publish(archive, manifest, rule, blocks=True)
                self.assertEqual(report["blocking_findings"], raw["blocking_findings"])
                self.assertEqual(report["package_evidence"]["entries"], [])

    def test_ambiguous_authority_unclosed_marker_and_wrong_context_refuse(self):
        ambiguous = b"https://" + b"fixture:example@candidate@sample.invalid/docs\n"
        for name, rule, data in (("ambiguous", URL_EXAMPLE_RULE, ambiguous),
                                 ("unclosed", MARKER_RULE, HEADER + b"\nQUJD\n"),
                                 ("wrong-context", URL_EXAMPLE_RULE, URL)):
            with self.subTest(case=name):
                archive, manifest, raw = self.fixture([data], [{rule: 1}])
                if name == "wrong-context":
                    manifest["entries"][0]["reviewed_literals"]["findings"][0]["spans"][0]["context"]["sha256"] = "f" * 64
                report = self.publish(archive, manifest, name, blocks=True)
                self.assertEqual(report["blocking_findings"], raw["blocking_findings"])
                self.assertEqual(report["package_evidence"]["entries"], [])

    def test_url_advisory_requires_a_complete_port_and_authority(self):
        authority = b"https://" + b"fixture:example@sample.invalid:"
        for name, port, blocks in (
            ("valid-port", b"65535", False),
            ("port-out-of-range", b"65536", True),
            ("port-letter-suffix", b"443x", True),
            ("second-port-separator", b"443:80", True),
        ):
            with self.subTest(case=name):
                archive, manifest, raw = self.fixture(
                    [authority + port + b"/docs\n"], [{URL_EXAMPLE_RULE: 1}]
                )
                self.assertEqual([row["rule"] for row in raw["blocking_findings"]], [URL_EXAMPLE_RULE])
                report = self.publish(archive, manifest, name, blocks=blocks)
                self.assertEqual(report["findings"], raw["findings"])
                self.assertEqual(
                    report["blocking_findings"], raw["blocking_findings"] if blocks else []
                )
                self.assertEqual(
                    [row["rule"] for row in report["package_evidence"]["entries"]],
                    [] if blocks else [URL_EXAMPLE_RULE],
                )

    def test_same_file_unrelated_finding_preserves_publication_refusal(self):
        archive, manifest, raw = self.fixture([URL + packages.PUBLIC_SOURCE], [{URL_EXAMPLE_RULE: 1}])
        report = self.publish(archive, manifest, "uncovered", blocks=True)
        self.assertEqual(report["findings"], raw["findings"])
        self.assertEqual([row["rule"] for row in report["blocking_findings"]], [packages.RULE])
        self.assertEqual([row["rule"] for row in report["package_evidence"]["entries"]], [URL_EXAMPLE_RULE])

    def test_native_newline_marker_needs_a_terminated_record(self):
        for name, suffix, blocks in (("native-lf-terminated", b"\n\0", False),
                                     ("native-lf-open", b"\ndata", True)):
            with self.subTest(boundary=name):
                archive, manifest, raw = self.fixture([b"native\0" + HEADER + suffix], [{MARKER_RULE: 1}])
                report = self.publish(archive, manifest, name, blocks=blocks)
                self.assertEqual(report["findings"], raw["findings"])
                self.assertEqual(len(report["package_evidence"]["entries"]), int(not blocks))

    def test_nested_links_cannot_supply_literal_proofs(self):
        archive, manifest, raw = self.fixture([URL], [{URL_EXAMPLE_RULE: 1}], nested=True, mode=stat.S_IFLNK)
        report = self.publish(archive, manifest, "link", blocks=True)
        self.assertEqual(report["blocking_findings"], raw["blocking_findings"])
        self.assertEqual(report["package_evidence"]["entries"], [])

    def test_nested_proof_does_not_clear_the_outer_payload_view(self):
        archive, manifest, _ = self.fixture([URL], [{URL_EXAMPLE_RULE: 1}], nested=True,
                                            compression=zipfile.ZIP_STORED)
        report = self.publish(archive, manifest, "uncovered-outer", blocks=True)
        self.assertEqual([(row["rule"], row.get("member")) for row in report["blocking_findings"]],
                         [(URL_EXAMPLE_RULE, None)])
        self.assertEqual([(row["rule"], row["member"]) for row in report["package_evidence"]["entries"]],
                         [(URL_EXAMPLE_RULE, "0")])

    def test_outer_and_nested_marker_proofs_apply_independently(self):
        archive, manifest, raw = self.fixture([MARKER], [{MARKER_RULE: 1}], nested=True,
                                            compression=zipfile.ZIP_STORED)
        with tarfile.open(archive) as saved:
            with tarfile.open(fileobj=saved.extractfile("layer0/layer.tar")) as layer:
                payload = layer.extractfile(WHEEL).read()
        [inner] = manifest["entries"]
        outer = dict(copy.deepcopy(inner), member=None)
        [outer_finding] = [row for row in raw["blocking_findings"] if row["rule"] == MARKER_RULE
                           and "member" not in row]
        [claim] = outer["reviewed_literals"]["findings"]
        claim["finding_id"] = outer_finding["finding_id"]
        [span] = claim["spans"]
        span["offset"] = payload.index(HEADER)
        span["context"] = {"offset": payload.index(MARKER), "size": len(MARKER),
                           "sha256": digest_bytes(MARKER)}
        manifest["entries"].append(outer)
        accepted = self.publish(archive, manifest, "both-views")
        self.assertEqual(accepted["findings"], raw["findings"])
        self.assertEqual({row["member"] for row in accepted["package_evidence"]["entries"]}, {None, "0"})
        for name, member, only in (("outer", "0", outer), ("inner", None, inner)):
            with self.subTest(view=name):
                manifest["entries"] = [only]
                refused = self.publish(archive, manifest, "only-" + name, blocks=True)
                self.assertEqual([row.get("member") for row in refused["blocking_findings"]], [member])

    def test_marker_cannot_clear_encoded_private_material_outside_the_context(self):
        key = subprocess.run(["openssl", "genpkey", "-algorithm", "EC", "-pkeyopt",
                              "ec_paramgen_curve:prime256v1", "-outform", "DER"],
                             check=True, capture_output=True, timeout=10).stdout
        pgp = bytes([0xC5, 32, 4]) + b"\0" * 31
        for name, candidate in (("DER", key), ("PGP", pgp)):
            with self.subTest(material=name):
                data = MARKER + b'"' + base64.b64encode(candidate) + b'"\n'
                archive, manifest, raw = self.fixture([data], [{MARKER_RULE: 1}])
                [claim] = manifest["entries"][0]["reviewed_literals"]["findings"]
                claim["spans"][0]["context"] = {"offset": 0, "size": len(MARKER), "sha256": digest_bytes(MARKER)}
                report = self.publish(archive, manifest, name, blocks=True)
                self.assertEqual(report["blocking_findings"], raw["blocking_findings"])
                self.assertEqual(report["package_evidence"]["entries"], [])

    def test_incomplete_encoded_inspection_keeps_the_marker_blocking(self):
        data = MARKER + base64.b64encode(b"z" * 32)
        archive, manifest, raw = self.fixture([data], [{MARKER_RULE: 1}])
        with patch.object(reviewed_literals, "MAX_ENCODED_TOTAL_BYTES", 1):
            report = self.publish(archive, manifest, "encoded-budget", blocks=True)
        self.assertEqual(report["blocking_findings"], raw["blocking_findings"])
        self.assertEqual(report["package_evidence"]["entries"], [])

    def test_marker_cannot_clear_line_wrapped_encoded_material(self):
        # The decoded public documentation contains a marker. Splitting its
        # encoding must not change the assessment of the surrounding file.
        documentation = b"z" * 44 + HEADER + b"z" * 50
        encoded = base64.b64encode(documentation)
        for name, body in (("contiguous", encoded), ("wrapped", base64.encodebytes(documentation)),
                           ("short-lines", b"\r\n".join(encoded[i:i + 12] for i in range(0, len(encoded), 12)))):
            with self.subTest(representation=name):
                data = MARKER + b'"' + body + b'"\n'
                archive, manifest, raw = self.fixture([data], [{MARKER_RULE: 1}])
                [claim] = manifest["entries"][0]["reviewed_literals"]["findings"]
                claim["spans"][0]["context"] = {"offset": 0, "size": len(MARKER), "sha256": digest_bytes(MARKER)}
                refused = self.publish(archive, manifest, name, blocks=True)
                self.assertEqual(refused["blocking_findings"], raw["blocking_findings"])
                self.assertEqual(refused["package_evidence"]["entries"], [])
