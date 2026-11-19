"""Bind publication to the reviewed scanner, explicit mode and exact archive."""
import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

from image_scanner_policy import (
    MANIFEST_SHA256, PolicyRefusal, load_classifications, load_content_policy, load_package_evidence, verify_bundle,
)
from image_scan_evidence import read as failure_evidence
from image_detector_cache import scanner_options

SCANNER = Path(__file__).resolve().parent / "image-secret-scanner" / "scan-image-secrets.sh"
EMPTY_POLICY = '{"version":1,"entries":[]}\n'
PUBLIC_CLASSES = frozenset({"library-example", "test-fixture", "public-material"})
RUNTIME_COMPONENTS = frozenset({"runtime", "runtime-omarchy"})


class ScanRefusal(ValueError):
    """Fixed diagnostics that never include scanner output or image content."""

    def __init__(self, message, outcome="scan_refused"):
        super().__init__(message)
        self.outcome = outcome
        self.evidence = None
        self.package_report = None


def checksum(path):
    result = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            result.update(block)
    return result.hexdigest()


def run_scan(*args):
    # Archive scans need no GitHub, registry, or deployment credential.
    env = {key: value for key, value in os.environ.items()
           if key in ("PATH", "HOME", "LANG", "LC_ALL", "TMPDIR", "RUNNER_TEMP")}
    try:
        result = subprocess.run(args, capture_output=True, timeout=1200, env=env, check=False)
    except subprocess.TimeoutExpired as error:
        raise ScanRefusal("Image secret scanner timed out; publication stopped", "scanner_timeout") from error
    except OSError as error:
        raise ScanRefusal("Image secret scanner unavailable; publication stopped", "scanner_unavailable") from error
    if result.returncode != 0:
        outcome = "scanner_findings" if result.returncode == 1 else "scanner_error"
        raise ScanRefusal("Image secret scan has blocking findings or did not complete; publication stopped", outcome)


def require(condition, message="Image secret scan receipt is incomplete, unbound, or has blockers; publication stopped"):
    if not condition:
        raise ScanRefusal(message)


def validate_arch_certificate(evidence, package_rows, archive_digest):
    from image_secrets import arch_trust
    from image_secrets.gate import ARCH_CERTIFICATE_ADVISORY, valid_digest

    proof = evidence["arch_trust"]
    path, base = proof["public_path"], proof["base"]
    require(evidence["platform"] == arch_trust.PLATFORM and evidence["layer"] == arch_trust.LAYER
            and valid_digest(evidence["proof_sha256"])
            and evidence["layer_digest"] == arch_trust.physical_layer(proof)
            and all(re.fullmatch(r"sha256:[a-f0-9]{64}", digest) for digest in base.values()))
    repack = proof.get("repack")
    if repack is not None:
        require(arch_trust.matches_image(proof, archive_digest, evidence["image_config"], evidence["image_manifest"])
                and repack["layer_diff_id"] != base["layer_diff_id"]
                and all(valid_digest(repack[key]) for key in
                        ("producer_sha256", "recipe_sha256", "attestation_sha256")))
    if path == arch_trust.ALIAS:
        target = package_rows.get(proof["target_finding_id"])
        require(proof["link_target"] == arch_trust.LINK and evidence["file_sha256"] is None
                and proof.get("namespace_verified") is True
                and target is not None and target["classification"] == ARCH_CERTIFICATE_ADVISORY
                and target["arch_trust"]["public_path"] == arch_trust.TLS and target["arch_trust"]["base"] == base
                and target["arch_trust"].get("repack") == repack
                and all(target[key] == evidence[key] for key in ("image", "image_config", "image_manifest", "layer")))
        return
    hashes = proof["der_sha256s"]
    require(path in arch_trust.FILES and proof["member_sha256"] == evidence["file_sha256"]
            and len(hashes) == arch_trust.FILES[path] and all(valid_digest(value) for value in hashes)
            and hashes == sorted(set(hashes)))
    native = proof.get("native")
    require((path == arch_trust.GNUPG) == (native is not None))
    if native is not None:
        require(native["package"]["authority_type"] == "arch-signature"
                and native["package"]["artifact_sha256"] == native["package_sha256"]
                and native["member"]["type"] == "regular" and native["member"]["path"][-1] == path
                and native["member"]["sha256"] == proof["member_sha256"] and valid_digest(native["mtree_sha256"]))


def validate_receipt(report, archive_digest, mode, packet, *, component=None, package_section=None):
    policy = report["policy"]
    expected_runtime = component if mode == "credential" and component in RUNTIME_COMPONENTS else None
    require(policy.get("staging_runtime_component") == expected_runtime)
    require(expected_runtime is None or packet is None)
    require(report.get("schema") == "image-secret-scan-v1" and report.get("scanner_version") == "1.0.4"
            and report.get("complete") is True and report.get("status") == "CLEAN"
            and type(report.get("exit_code")) is int and report["exit_code"] == 0 and not report.get("error")
            and report.get("blocking_rules") == mode and report.get("blocking_findings") == []
            and report.get("allowlisted") == [] and report.get("gitleaks_version") == "8.30.1"
            and policy.get("allowlist_sha256") == hashlib.sha256(EMPTY_POLICY.encode()).hexdigest()
            and policy.get("gitleaks_rules_sha256") == checksum(SCANNER.parent / "gitleaks.toml")
            and report.get("archive_sha256") == archive_digest)
    decisions = {}
    package_rows = {}
    proved_public_ids = set()
    if package_section is None:
        require("package_evidence" not in report and "package_evidence_sha256" not in policy)
    else:
        from image_secrets import nested_certificates, package_receipt, reviewed_literals
        from image_secrets.gate import (
            ARCH_CERTIFICATE_ADVISORY, CERTIFICATE_ADVISORY, DEBIAN_CONTROL_ADVISORY,
            DIGEST_FRAME_ADVISORY, DISTRIBUTION_ADVISORY,
            KEY_MARKER_VIEW_ADVISORY, PACKAGE_ADVISORY_RULES,
            EXECUTABLE_TEST_ADVISORY, SELF_TEST_ADVISORY, package_advisory_allowed, valid_digest,
        )

        require(packet is None and expected_runtime is not None
                and report.get("package_evidence") == package_section
                and package_section["component"] == expected_runtime and package_section["scope"] == "staging"
                and package_section["scanner_version"] == report["scanner_version"]
                and package_section["scanner_manifest_sha256"] == MANIFEST_SHA256
                and package_section["archive_sha256"] == archive_digest
                and policy.get("package_evidence_sha256") == package_receipt.digest(package_section))
        decisions = package_receipt.decisions(package_section)
        package_rows = {row["finding_id"]: row for row in package_section["entries"]}
        for evidence in package_rows.values():
            classification = evidence["classification"]
            origin = evidence.get("derived_payload")
            if origin is not None:
                require(origin["archive_sha256"] == archive_digest
                        and all(origin[key] == evidence[key] for key in ("image_config", "image_manifest"))
                        and evidence["finding_id"] in origin["findings"]
                        and origin["output_sha256"] == evidence["outer_file_sha256"]
                        and origin["input_sha256"] == evidence["package_member"]["sha256"]
                        and evidence["package_member"]["type"] == "regular")
                if origin["kind"] == "compiled-executable":
                    require(origin["span"]["sha256"] == evidence["candidate_sha256"])
            if classification in PACKAGE_ADVISORY_RULES:
                require(package_advisory_allowed(classification, evidence["rule"], expected_runtime, evidence["member"]))
                if classification == SELF_TEST_ADVISORY:
                    require(bool(evidence["library_self_test"]["vectors"]))
                elif classification == EXECUTABLE_TEST_ADVISORY:
                    executable = evidence["executable_self_test"]
                    require(bool(executable["vectors"]) and evidence["package_member"]["type"] == "regular"
                            and evidence["package_member"]["sha256"] == evidence["file_sha256"]
                            and package_receipt.path_digest(executable["executable_path"]) == evidence["outer_path_digest"])
                elif classification == DIGEST_FRAME_ADVISORY:
                    require(bool(evidence["public_digest_frames"]["frames"]))
                elif classification == DEBIAN_CONTROL_ADVISORY:
                    control = evidence["debian_control"]
                    require(control["archive_sha256"] == archive_digest
                            and all(control[key] == evidence[key] for key in ("image_config", "image_manifest", "finding_id"))
                            and control["view_sha256"] == evidence["file_sha256"]
                            and control["span"]["sha256"] == evidence["candidate_sha256"]
                            and evidence["package"]["authority_type"] == "debian-index"
                            and evidence["package_member"]["type"] == "regular"
                            and evidence["package_member"]["sha256"] == evidence["outer_file_sha256"]
                            and package_receipt.path_digest(control["installed_path"]) == evidence["outer_path_digest"])
                elif classification in reviewed_literals.CLASSES:
                    literal = evidence["reviewed_literal"]
                    source_sha256 = origin["input_sha256"] if origin is not None else evidence["outer_file_sha256"]
                    require(literal["archive_sha256"] == archive_digest and bool(literal["spans"])
                            and all(literal[key] == evidence[key] for key in
                                    ("image_config", "image_manifest", "finding_id", "rule"))
                            and evidence["package_member"]["type"] == "regular"
                            and evidence["package_member"]["sha256"] == source_sha256)
                elif classification == KEY_MARKER_VIEW_ADVISORY:
                    key_view = evidence["key_marker_view"]
                    require(key_view["archive_sha256"] == archive_digest and key_view["original"] == key_view["detector"]
                            and key_view["original"]["sha256"] == evidence["candidate_sha256"]
                            and all(key_view[key] == evidence[key] for key in ("image_config", "image_manifest", "finding_id"))
                            and evidence["package"]["authority_type"] == "arch-signature"
                            and evidence["package_member"]["type"] == "regular"
                            and evidence["package_member"]["sha256"] == evidence["outer_file_sha256"])
                elif classification == DISTRIBUTION_ADVISORY:
                    distribution = evidence["distribution_config"]
                    require(distribution["archive_sha256"] == archive_digest
                            and all(distribution[key] == evidence[key] for key in
                                    ("image_config", "image_manifest", "finding_id"))
                            and distribution["span"]["sha256"] == evidence["candidate_sha256"]
                            and distribution["provenance_sha256"] == evidence["proof_sha256"]
                            and evidence["package_member"]["type"] == "regular"
                            and evidence["package_member"]["sha256"] == evidence["file_sha256"]
                            and package_receipt.path_digest(distribution["public_path"]) == evidence["outer_path_digest"])
                elif classification == CERTIFICATE_ADVISORY:
                    cert = evidence["nested_certificate"]
                    hashes = cert["der_sha256s"]
                    require(cert["member_sha256"] == evidence["file_sha256"]
                            and cert["wheel_path"] == nested_certificates.WHEEL
                            and cert["member_path"] == nested_certificates.MEMBER
                            and valid_digest(cert["pypi_attestation_sha256"])
                            and len(hashes) == nested_certificates.COUNT
                            and all(valid_digest(value) for value in hashes) and hashes == sorted(set(hashes)))
                elif classification == ARCH_CERTIFICATE_ADVISORY:
                    validate_arch_certificate(evidence, package_rows, archive_digest)
                proved_public_ids.add(evidence["finding_id"])
    if packet is None:
        require("classifications_sha256" not in policy and "classification_review_sha256" not in policy)
    else:
        parsed = json.loads(packet)
        require(mode == "credential" and policy.get("classifications_sha256") == hashlib.sha256(packet).hexdigest()
                and policy.get("classification_review_sha256") == parsed["review"]["report_sha256"])
        decisions = {entry["finding_id"]: entry["classification"] for entry in parsed["entries"]}
        require(len(decisions) == len(parsed["entries"]))
    findings = report["findings"]
    require(isinstance(findings, list))
    observed, advisory, reviewed = set(), [], []
    for finding in findings:
        identity = finding["finding_id"]
        require(isinstance(identity, str) and re.fullmatch(r"[a-f0-9]{64}", identity)
                and identity not in observed)
        observed.add(identity)
        classification = decisions.get(identity)
        if classification is not None:
            proved_public = identity in proved_public_ids
            require(proved_public or (classification in PUBLIC_CLASSES and finding["rule"] not in {
                "content.private-key-header", "content.binary-private-key", "content.openpgp-private-key",
                "gitleaks.private-key", "path.ssh-key"}))
            if identity in package_rows:
                evidence = package_rows[identity]
                require(finding["scope"] == "layer" and evidence["path_digest"] == finding["path"]
                        and all(evidence[key] == finding.get(key) for key in (
                            "rule", "layer", "entry", "member", "image", "candidate_sha256", "line"))
                        and (evidence["file_sha256"] == finding.get("file_sha256")
                             or (proved_public and finding["rule"] != "gitleaks.private-key"
                                 and "file_sha256" not in finding)))
                raw = {key: value for key, value in finding.items() if key != "finding_id"}
                require(hashlib.sha256(json.dumps(raw, sort_keys=True, separators=(",", ":")).encode())
                        .hexdigest() == identity)
            (advisory if proved_public else reviewed).append(dict(finding, classification=classification))
        else:
            require(mode == "credential")
            if finding["rule"] == "path.key-or-certificate":
                digest = finding.get("public_certificate_sha256")
                body = {key: value for key, value in finding.items() if key != "finding_id"}
                require(expected_runtime is not None and isinstance(digest, str) and re.fullmatch(r"[a-f0-9]{64}", digest)
                        and finding.get("scope") == "layer" and "member" not in finding
                        and all(type(finding.get(k)) is int and finding[k] >= 0 for k in ("image", "layer", "entry"))
                        and isinstance(finding.get("path"), str)
                        and re.fullmatch(r"path-sha256:[a-f0-9]{64}", finding["path"])
                        and finding.get("file_sha256", digest) == finding.get("candidate_sha256", digest) == digest
                        and ("file_sha256" in finding) == ("candidate_sha256" in finding)
                        and hashlib.sha256(json.dumps(body, sort_keys=True, separators=(",", ":")).encode())
                        .hexdigest() == identity)
                advisory.append(dict(finding, classification="public-material"))
            else:
                require(finding["rule"] == "gitleaks.image-high-entropy")
                advisory.append(finding)
    require(set(decisions).issubset(observed) and report.get("advisory_findings") == advisory
            and report.get("reviewed_classifications") == reviewed)
    counts = {"detected": len(findings), "blocking": 0, "advisory": len(advisory),
              "reviewed": len(reviewed), "allowlisted": 0}
    require(report.get("counts") == counts and all(type(value) is int for value in report["counts"].values()))


def _verified_scan(archive, output, run_command, *, blocking_rules, component):
    try:
        return _scan_archive(archive, output, run_command, blocking_rules=blocking_rules, component=component)
    except ScanRefusal as error:
        error.evidence = failure_evidence(output / "REPORT.json", error.outcome)
        raise


def _scan_archive(archive, output, run_command, *, blocking_rules, component):
    require(blocking_rules in ("all", "credential"), "Unapproved image scan mode; publication stopped")
    require(not output.exists(), "Image secret scan output must be new; publication stopped")
    original = checksum(archive)
    runtime = blocking_rules == "credential" and component in RUNTIME_COMPONENTS
    try:
        root = Path(__file__).resolve().parents[2]
        bundle = MANIFEST_SHA256
        verify_bundle(root, manifest_sha256=bundle)
        packet = load_classifications(root, original) if blocking_rules == "credential" and not runtime else None
        content = load_content_policy(root) if blocking_rules == "credential" and component in {
            "web", "am", "eg"} else None
        evidence = load_package_evidence(root) if runtime else None
    except (PolicyRefusal, OSError, ValueError, KeyError, TypeError) as error:
        raise ScanRefusal("Image scanner or classification packet differs from the pinned review; publication stopped") from error
    output.parent.mkdir(parents=True, exist_ok=True)
    if component in RUNTIME_COMPONENTS:
        try:
            from runtime_image_hygiene import check_archive
            hygiene = output.with_name(output.name + "-runtime-hygiene.json")
            check_archive(archive, component, hygiene)
        except Exception as error:
            raise ScanRefusal("Runtime layer hygiene refused; publication stopped") from error
    # Reviewed classifications are artifact-bound. The generic scanner allowlist
    # stays empty so it cannot hide candidates before the Policy A decision.
    with tempfile.TemporaryDirectory(prefix="image-scan-policy-", dir=output.parent) as policy_dir:
        policy = Path(policy_dir) / "no-exceptions.json"
        policy.write_text(EMPTY_POLICY, encoding="utf-8")
        options = ["--blocking-rules", blocking_rules]
        if runtime:
            options += ["--staging-runtime-component", component]
        if packet is not None:
            classifications = Path(policy_dir) / "classifications.json"
            classifications.write_bytes(packet)
            options += ["--classifications", str(classifications)]
        options += scanner_options(root, os.environ, blocking_rules, packet)
        try:
            run_command("bash", str(SCANNER), str(archive), "--input-kind", "archive",
                        "--output", str(output), "--allowlist", str(policy),
                        "--expected-archive-sha256", original, *options)
        except ScanRefusal as error:
            if error.outcome != "scanner_findings" or not (content or evidence) or packet is not None:
                raise
            if content:
                try:
                    # Both detector and resolver bytes were verified before importing.
                    sys.path.insert(0, str(SCANNER.parent))
                    from image_content_policy import resolve
                    catalog, review_digest = content
                    packet = resolve(archive, output / "REPORT.json", component, catalog, review_digest, SCANNER.parent)
                except Exception as cause:
                    raise ScanRefusal("Content evidence refused; publication stopped", "content_refused") from cause
    package_report = None
    if evidence is not None:
        try:
            package_report = resolve_package_scan(archive, output, component, evidence, bundle)
        except Exception as cause:
            raise ScanRefusal("Package evidence refused; publication stopped", "content_refused") from cause
    try:
        report = package_report or json.loads((output / "REPORT.json").read_text())
        package_section = package_report["package_evidence"] if package_report is not None else None
        if (packet is not None or package_section is not None) and report.get("complete") is True and report.get("status") == "FINDINGS":
            refusal = ScanRefusal("Unresolved image content remains blocking", "scanner_findings")
            refusal.evidence = failure_evidence(output / "REPORT.json", refusal.outcome)
            refusal.package_report = package_report
            raise refusal
        validate_receipt(report, original, blocking_rules, packet, component=component, package_section=package_section)
        require(checksum(archive) == original)
    except ScanRefusal:
        raise
    except (OSError, ValueError, AttributeError, KeyError, TypeError) as error:
        raise ScanRefusal("Image secret scan receipt is missing or invalid; publication stopped", "receipt_invalid") from error
    counts = report["counts"]
    print(f"Image secret scan passed: mode={blocking_rules}, blockers=0, "
          f"advisory={counts['advisory']}, reviewed={counts['reviewed']}, archive=sha256:{original}")
    return original, (report["scanner_version"], bundle)


def resolve_package_scan(archive, output, component, evidence, bundle):
    root = Path(__file__).resolve().parents[2]
    require(load_content_policy(root) is not None, "Package report validator has no pinned review")
    sys.path.insert(0, str(SCANNER.parent))
    from image_content_policy import validate_detection
    from image_secrets import package_receipt
    from image_secrets.common import json_bytes
    from image_secrets.gate import assess
    from image_secrets.package_evidence import PackageEvidence

    report_path = output / "REPORT.json"
    report = json_bytes(report_path.read_bytes())
    validate_detection(report, archive, SCANNER.parent)
    raw, manifest, review = evidence
    policy = PackageEvidence.load(raw, manifest_sha256=manifest, review_sha256=review,
                                  scanner_manifest_sha256=bundle)
    section = package_receipt.build(archive, output, report, component, policy)
    assess(report, "credential", verified_package_decisions=package_receipt.decisions(section))
    report["package_evidence"] = section
    report["policy"]["package_evidence_sha256"] = package_receipt.digest(section)
    report["exit_code"] = 1 if report["blocking_findings"] else 0
    report_path.write_text(json.dumps(report, sort_keys=True) + "\n")
    return report


def scan_archive(archive, output, run_command=run_scan, *, blocking_rules="all", component=None):
    digest, _ = _verified_scan(archive, output, run_command, blocking_rules=blocking_rules, component=component)
    return digest


def verified_staging_scan(archive, output, run_command=run_scan, *, component):
    return _verified_scan(archive, output, run_command, blocking_rules="credential", component=component)


def scan_staging_archive(archive, output, run_command=run_scan):
    """The reviewed staging-track contract explicitly selects Policy A."""
    digest, _ = verified_staging_scan(archive, output, run_command, component=os.environ.get("CD_COMPONENT"))
    return digest
