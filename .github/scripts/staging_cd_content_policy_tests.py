"""Exact content decisions cannot approve altered bytes or incomplete scans."""
import contextlib
import copy
import hashlib
import io
import json
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import image_scanner_policy as pins
import image_secret_gate as gate
from image_scan_evidence import project
from staging_cd_publication_diagnostics import run_publication

SCANNER = Path(__file__).parent / "image-secret-scanner"
sys.path.insert(0, str(SCANNER))
from image_content_identity import archive_identities, layer_identities
from image_content_policy import catalog_index, decisions, path_hash, validate_detection
from image_secrets.common import ScanError
from image_secrets.gate import assess
from tests.fixtures import docker_archive, tar_bytes


def digest(data):
    return hashlib.sha256(data).hexdigest()


def content_image(path, payload):
    # The same synthetic image exercises content resolution for web and runtime.
    entries = [("etc/example.pem", payload), ("etc/machine-id", b""),
               ("var/lib/dbus/machine-id", (tarfile.SYMTYPE, "/etc/machine-id"))]
    return docker_archive(path, [tar_bytes(entries)])


def finding(path, payload=None, **changes):
    row = {"scope": "layer", "image": 0, "layer": 0, "entry": 0, "path": path_hash(path),
           "rule": "path.key-or-certificate"}
    if payload is not None:
        row.update(file_sha256=digest(payload), candidate_sha256=digest(payload), line=1)
    row.update(changes)
    row["finding_id"] = digest(json.dumps(row, sort_keys=True, separators=(",", ":")).encode())
    return row


class ContentPolicyTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.archive = self.root / "image.tar"
        self.output = self.root / "scan"
        self.payload = b"synthetic public payload"
        self.item = finding("etc/example.pem", self.payload)
        config = content_image(self.archive, self.payload)
        files = [SCANNER / "scan-image-secrets.sh", SCANNER / "scan_image_secrets.py"]
        files += sorted((SCANNER / "image_secrets").glob("*.py"))
        self.report = {"schema": "image-secret-scan-v1", "scanner_version": "1.0.4", "gitleaks_version": "8.30.1",
                       "complete": True, "input_kind": "archive", "archive_sha256": gate.checksum(self.archive),
                       "findings": [self.item], "allowlisted": [], "exit_code": 1,
                       "scanner_files": {str(p.relative_to(SCANNER)): gate.checksum(p) for p in files},
                       "inventory": {"images": [{"config_digest": "sha256:" + config}]},
                       "policy": {"allowlist_sha256": digest(gate.EMPTY_POLICY.encode()),
                                  "gitleaks_rules_sha256": gate.checksum(SCANNER / "gitleaks.toml")}}
        assess(self.report, "credential")
        self.entry = {"component": "web", "scope": "layer", "path": "etc/example.pem", "file_sha256": digest(self.payload),
                      "candidate_sha256": digest(self.payload), "rule": self.item["rule"],
                      "classification": "public-material", "reason": "Synthetic fixture only"}
        self.catalog = {"schema": "cma-image-content-policy-v1", "scanner_version": "1.0.4",
                        "gitleaks_version": "8.30.1", **self.report["policy"], "entries": [self.entry]}
        self.catalog_path = self.root / "catalog.json"
        self.catalog_path.write_text(json.dumps(self.catalog))

    def scanner(self, *args):
        self.output.mkdir()
        report = copy.deepcopy(self.report)
        if "--staging-runtime-component" in args:
            report["policy"]["staging_runtime_component"] = args[args.index("--staging-runtime-component") + 1]
        (self.output / "REPORT.json").write_text(json.dumps(report))
        raise gate.ScanRefusal("Synthetic findings", "scanner_findings")

    def test_complete_scan_resolves_exact_content_and_keeps_all_findings(self):
        with patch.object(gate, "load_content_policy", return_value=(self.catalog_path, "a" * 64)):
            actual = gate.scan_archive(self.archive, self.output, self.scanner, blocking_rules="credential", component="web")
        self.assertEqual(actual, gate.checksum(self.archive))
        report = json.loads((self.output / "REPORT.json").read_text())
        self.assertEqual(report["findings"], [self.item])
        self.assertEqual(report["counts"], {"detected": 1, "blocking": 0, "advisory": 0, "reviewed": 1, "allowlisted": 0})

    def test_unapproved_content_never_permits_publication(self):
        with patch.object(gate, "load_content_policy", return_value=None), self.assertRaises(gate.ScanRefusal) as caught:
            gate.scan_archive(self.archive, self.output, self.scanner, blocking_rules="credential", component="web")
        self.assertEqual(caught.exception.evidence["counts"]["blocking"], 1)

    def test_unknown_and_real_candidates_still_refuse_after_resolution(self):
        for entries in ([], [dict(self.entry, classification="REAL")]):
            self.catalog_path.write_text(json.dumps(dict(self.catalog, entries=entries)))
            with patch.object(gate, "load_content_policy", return_value=(self.catalog_path, "a" * 64)), \
                    self.assertRaises(gate.ScanRefusal):
                gate.scan_archive(self.archive, self.output, self.scanner, blocking_rules="credential", component="web")
            report = json.loads((self.output / "REPORT.json").read_text())
            self.assertEqual(report["counts"]["blocking"], 1)
            for path in self.output.iterdir():
                path.unlink()
            self.output.rmdir()

    def test_timeout_and_process_error_never_enter_content_resolution(self):
        for outcome in ("scanner_timeout", "scanner_error", "scanner_unavailable"):
            with patch.object(gate, "load_content_policy", return_value=(self.catalog_path, "a" * 64)), \
                    patch("image_content_policy.resolve") as resolver:
                def failed(*_args):
                    raise gate.ScanRefusal("fixture", outcome)
                with self.assertRaises(gate.ScanRefusal) as caught:
                    gate.scan_archive(self.archive, self.output, failed, blocking_rules="credential", component="web")
                self.assertEqual(caught.exception.evidence["outcome"], outcome)
                resolver.assert_not_called()

    def test_component_path_rule_value_payload_and_unknown_findings_refuse(self):
        identities = {self.item["finding_id"]: digest(self.payload)}
        for field, value in (("component", "am"), ("path", "etc/other.pem"), ("rule", "gitleaks.generic-api-key"),
                             ("candidate_sha256", "b" * 64), ("file_sha256", "b" * 64)):
            catalog = copy.deepcopy(self.catalog)
            catalog["entries"][0][field] = value
            with self.subTest(field=field):
                self.assertEqual(decisions(catalog_index(catalog, self.report), "web", [self.item], identities), [])
        self.assertEqual(decisions({}, "web", [self.item], identities), [])

    def binary_case(self, component="am", path="usr/local/bin/cma"):
        candidate = digest(b"adjacent public rust text")
        item = finding(path, b"rebuilt first-party binary", rule="gitleaks.generic-api-key",
                       candidate_sha256=candidate)
        row = {"component": component, "path": path, "rule": "gitleaks.generic-api-key",
               "candidate_sha256": candidate, "classification": "public-material",
               "reason": "Synthetic first-party binary text"}
        catalog = dict(copy.deepcopy(self.catalog), schema="cma-image-content-policy-v2",
                       binary_candidates=[row])
        return item, row, catalog

    def test_first_party_binary_candidate_survives_a_rebuilt_file_digest(self):
        for component, path in (("am", "usr/local/bin/cma"), ("eg", "usr/local/bin/cmaeg")):
            with self.subTest(component=component):
                item, _row, catalog = self.binary_case(component, path)
                matched = decisions(catalog_index(catalog, self.report), component, [item],
                                    {item["finding_id"]: item["file_sha256"]})
                self.assertEqual([(m["finding_id"], m["classification"]) for m in matched],
                                 [(item["finding_id"], "public-material")])

    def test_first_party_binary_candidate_is_bound_to_component_path_rule_value_and_identity(self):
        item, _row, catalog = self.binary_case()
        index = catalog_index(catalog, self.report)
        identities = {item["finding_id"]: item["file_sha256"]}
        cases = {
            "component": ("eg", [item], identities),
            "path": ("am", [finding("usr/local/bin/other", b"rebuilt first-party binary",
                                    rule="gitleaks.generic-api-key",
                                    candidate_sha256=item["candidate_sha256"])], None),
            "rule": ("am", [finding("usr/local/bin/cma", b"rebuilt first-party binary",
                                    rule="gitleaks.image-secret-assignment",
                                    candidate_sha256=item["candidate_sha256"])], None),
            "candidate": ("am", [finding("usr/local/bin/cma", b"rebuilt first-party binary",
                                         rule="gitleaks.generic-api-key", candidate_sha256="c" * 64)], None),
            "identity": ("am", [item], {item["finding_id"]: "d" * 64}),
            "unverified": ("am", [item], {}),
        }
        for name, (component, findings, known) in cases.items():
            with self.subTest(name=name):
                known = known if known is not None else {f["finding_id"]: f["file_sha256"] for f in findings}
                self.assertEqual(decisions(index, component, findings, known), [])

    def test_first_party_binary_rows_refuse_widening_and_real_conflicts(self):
        _item, row, catalog = self.binary_case()
        broken = [dict(row, path="usr/local/bin/cmaeg"), dict(row, component="web"),
                  dict(row, rule="gitleaks.image-secret-assignment"), dict(row, classification="REAL"),
                  dict(row, candidate_sha256="not-a-digest"), dict(row, reason=" "), dict(row, extra=True),
                  {key: value for key, value in row.items() if key != "reason"}]
        for value in broken:
            with self.subTest(row=sorted(value)), self.assertRaises(ScanError):
                catalog_index(dict(catalog, binary_candidates=[value]), self.report)
        with self.assertRaises(ScanError):
            catalog_index(dict(catalog, binary_candidates=[row, dict(row)]), self.report)
        with self.assertRaises(ScanError):
            catalog_index(dict(catalog, binary_candidates=[row] * 17), self.report)
        real = dict(self.entry, component="am", rule=row["rule"], candidate_sha256=row["candidate_sha256"],
                    classification="REAL")
        with self.assertRaises(ScanError):
            catalog_index(dict(catalog, entries=[*catalog["entries"], real]), self.report)
        with self.assertRaises(ScanError):
            catalog_index(dict(catalog, schema="cma-image-content-policy-v1"), self.report)

    def test_reviewed_agent_manager_binary_candidate_is_exact_and_narrow(self):
        catalog = json.loads((Path(__file__).parent / "image-content-policy.json").read_text())
        self.assertEqual(catalog["binary_candidates"], [{
            "component": "am", "path": "usr/local/bin/cma", "rule": "gitleaks.generic-api-key",
            "candidate_sha256": "eee389a1fcecdde9bd6383b996396c5559dbf3b420ed5d80cead274341b0ab39",
            "classification": "public-material", "reason": catalog["binary_candidates"][0]["reason"]}])

    def test_real_decision_overrides_matching_public_material(self):
        self.catalog["entries"].append(dict(self.entry, classification="REAL"))
        matched = decisions(catalog_index(self.catalog, self.report), "web", [self.item],
                            {self.item["finding_id"]: digest(self.payload)})
        self.assertEqual(matched[0]["classification"], "REAL")

    def test_incomplete_stale_forged_or_changed_report_refuses(self):
        for changes in ({"complete": False}, {"error": "fixture"}, {"archive_sha256": "a" * 64},
                        {"scanner_files": {}}, {"counts": {}}, {"blocking_findings": []},
                        {"exit_code": 0}, {"allowlisted": [{}]}, {"findings": [self.item, self.item]},
                        {"findings": [dict(self.item, rule="unknown")]}):
            with self.subTest(changes=changes), self.assertRaises(ScanError):
                validate_detection(dict(self.report, **changes), self.archive, SCANNER)

    def test_links_whiteouts_duplicate_entries_and_hardlinks_keep_exact_identity(self):
        cases = [([("target", self.payload), ("etc/example.pem", (tarfile.SYMTYPE, "../target"))], 1, digest(self.payload)),
                 ([("target", self.payload), ("etc/example.pem", (tarfile.LNKTYPE, "target"))], 1, None),
                 ([("etc/example.pem", self.payload), ("etc/example.pem", b"replacement")], 0, digest(self.payload)),
                 ([(".wh.target", b""), ("etc/example.pem", (tarfile.SYMTYPE, "../target"))], 1, None),
                 ([("etc/example.pem", (tarfile.SYMTYPE, "example.pem"))], 0, None)]
        for entries, number, expected in cases:
            layer = self.root / "layer.tar"
            layer.write_bytes(tar_bytes(entries))
            item = finding("etc/example.pem", entry=number)
            result = layer_identities(layer, {"target": ("file", digest(self.payload))}, {number: [item]})
            self.assertEqual(result.get(item["finding_id"]), expected)

    def test_layer_resolution_keeps_backslash_and_plain_names_distinct(self):
        escaped = r"usr/lib/systemd/system/example\x2dunit"
        plain = "usr/lib/systemd/system/example-unit"
        layer = self.root / "layer.tar"
        layer.write_bytes(tar_bytes([(escaped, self.payload), (plain, b"other")]))
        exact = finding(escaped, self.payload)
        wrong = finding(plain, self.payload)
        self.assertEqual(layer_identities(layer, {}, {0: [exact, wrong]}),
                         {exact["finding_id"]: digest(self.payload)})

    def test_pax_credential_metadata_cannot_inherit_public_payload(self):
        layer = self.root / "layer.tar"
        with tarfile.open(layer, "w", format=tarfile.PAX_FORMAT) as tar:
            member = tarfile.TarInfo("etc/example.pem")
            member.size = len(self.payload)
            member.pax_headers = {"comment": "https://" + "fixture" + ":" + "nonfunctional" + "@example.invalid"}
            tar.addfile(member, io.BytesIO(self.payload))
        item = finding("etc/example.pem", rule="content.credential-url")
        self.assertEqual(layer_identities(layer, {}, {0: [item]}), {})

    def test_config_history_requires_exact_decoded_payload_and_config_digest(self):
        payload = b"synthetic public checksum"
        item = finding("config/history/0/created_by", payload, scope="config", layer=-1, entry=-1,
                       rule="gitleaks.generic-api-key")
        config = docker_archive(self.archive, [], extra={"history": [{"created_by": payload.decode()}]})
        report = {"findings": [item], "inventory": {"images": [{"config_digest": "sha256:" + config}]}}
        workspace = self.root / "identity"
        workspace.mkdir()
        self.assertEqual(archive_identities(self.archive, workspace, report), {item["finding_id"]: digest(payload)})
        config = docker_archive(self.archive, [], extra={"history": [{"created_by": "changed content"}]})
        with self.assertRaises(ScanError):
            archive_identities(self.archive, workspace, report)
        report["inventory"]["images"][0]["config_digest"] = "sha256:" + config
        self.assertEqual(archive_identities(self.archive, workspace, report), {})

    def test_manifest_pin_requires_exact_reviewed_catalog_and_sources(self):
        directory = self.root / ".github/scripts"
        directory.mkdir(parents=True)
        for name in pins.CONTENT_FILES:
            (directory / name).parent.mkdir(parents=True, exist_ok=True)
            (directory / name).write_text("fixture")
        manifest = directory / "image-content-manifest.json"
        manifest.write_text(json.dumps({"schema": "cma-image-content-source-v1",
                                       "files": {name: gate.checksum(directory / name) for name in pins.CONTENT_FILES}}))
        with patch.object(pins, "APPROVED_CONTENT_POLICY", (gate.checksum(manifest), "a" * 64)):
            self.assertEqual(pins.load_content_policy(self.root), (directory / "image-content-policy.json", "a" * 64))
            (directory / "image-content-policy.json").write_text("changed")
            with self.assertRaises(pins.PolicyRefusal):
                pins.load_content_policy(self.root)

    def test_failure_projection_rejects_values_paths_and_injected_output(self):
        private = "private-value\n::warning::injected"
        report = dict(self.report, error=private, secret=private)
        report["blocking_findings"] = [dict(self.item, path=private, rule=private, Secret=private)]
        error = gate.ScanRefusal(private, "scanner_findings")
        error.evidence = project(report, error.outcome)
        def fail(_masker, diagnostics):
            diagnostics.mark("scan")
            raise error
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            self.assertEqual(run_publication(fail), 1)
        self.assertNotIn(private, output.getvalue())
        self.assertIn('"outcome": "scanner_findings"', output.getvalue())
        self.assertIn('"rule": "unrecognized_rule"', output.getvalue())
        self.assertIn(self.item["finding_id"], output.getvalue())

    def test_malformed_failure_fields_and_large_receipts_stay_bounded(self):
        report = {"complete": False, "counts": [], "findings": [{"rule": [], "scope": []}] * 1001}
        evidence = project(report, [])
        self.assertEqual(len(evidence["findings"]), 1000)
        self.assertEqual(evidence["omitted_findings"], 1)
        self.assertEqual(project(evidence, "scan_refused")["omitted_findings"], 1)


if __name__ == "__main__":
    unittest.main()
