"""Classify exact reviewed content after a complete, unchanged detector pass."""
import copy
import hashlib
import json
import tempfile
from pathlib import Path

from image_secrets.common import ScanError, archive_path, digest_file
from image_secrets.gate import assess, valid_digest

PUBLIC_RULES = frozenset({"path.key-or-certificate", "gitleaks.image-secret-assignment",
                          "gitleaks.generic-api-key"})


def require(condition):
    if not condition:
        raise ScanError("content-policy-evidence-invalid")


def path_hash(path):
    return "path-sha256:" + hashlib.sha256(path.encode("utf-8", "surrogatepass")).hexdigest()


def validate_detection(report, archive, scanner):
    require(report.get("schema") == "image-secret-scan-v1" and report.get("complete") is True
            and report.get("status") in {"CLEAN", "FINDINGS"} and not report.get("error")
            and report.get("blocking_rules") == "credential" and report.get("input_kind") == "archive"
            and not report.get("reviewed_classifications") and report.get("allowlisted") == []
            and report.get("archive_sha256") == digest_file(archive))
    files = [scanner / "scan-image-secrets.sh", scanner / "scan_image_secrets.py"]
    files += sorted((scanner / "image_secrets").glob("*.py"))
    require(report.get("scanner_files") == {str(p.relative_to(scanner)): digest_file(p) for p in files})
    seen = set()
    for finding in report["findings"]:
        identity = finding["finding_id"]
        body = {key: value for key, value in finding.items() if key != "finding_id"}
        require(valid_digest(identity) and identity not in seen
                and hashlib.sha256(json.dumps(body, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
                == identity)
        seen.add(identity)
    expected = copy.deepcopy(report)
    assess(expected, "credential")
    for field in ("status", "counts", "blocking_findings", "advisory_findings", "reviewed_classifications"):
        require(report.get(field) == expected[field])
    require(all(type(value) is int for value in report["counts"].values()))
    require(type(report.get("exit_code")) is int
            and report["exit_code"] == (1 if report["blocking_findings"] else 0))


def catalog_index(catalog, report):
    require(catalog.get("schema") == "cma-image-content-policy-v1")
    for key in ("scanner_version", "gitleaks_version"):
        require(catalog.get(key) == report.get(key))
    for key in ("allowlist_sha256", "gitleaks_rules_sha256"):
        require(catalog.get(key) == report["policy"].get(key))
    index = {}
    for row in catalog["entries"]:
        require(row["component"] in {"web", "am", "eg"} and row["scope"] in {"layer", "config"}
                and archive_path(row["path"]) == row["path"] and "*" not in row["path"]
                and valid_digest(row["file_sha256"]) and row["classification"] in {"public-material", "REAL"}
                and isinstance(row["reason"], str) and bool(row["reason"].strip())
                and (row["classification"] == "REAL" or row["rule"] in PUBLIC_RULES))
        require("candidate_sha256" not in row or valid_digest(row["candidate_sha256"]))
        require(row["rule"] == "path.key-or-certificate" or "candidate_sha256" in row)
        key = (row["component"], row["scope"], path_hash(row["path"]), row["rule"], row["file_sha256"])
        index.setdefault(key, []).append(row)
    return index


def decisions(index, component, findings, identities):
    matched = []
    for finding in findings:
        key = (component, finding["scope"], finding["path"], finding["rule"],
               identities.get(finding["finding_id"]))
        rows = [row for row in index.get(key, [])
                if all(finding.get(field) == row[field] for field in ("candidate_sha256", "line") if field in row)]
        if not rows:
            continue
        # A REAL decision always blocks; a competing public entry cannot override it.
        if any(row["classification"] == "REAL" for row in rows):
            classification, reason = "REAL", "Reviewed secret content remains blocking"
        else:
            choices = {(row["classification"], row["reason"]) for row in rows}
            require(len(choices) == 1)
            classification, reason = choices.pop()
        matched.append({"finding_id": finding["finding_id"], "classification": classification, "reason": reason})
    return matched


def resolve(archive, report_path, component, catalog_path, review_sha256, scanner):
    from image_content_identity import archive_identities

    report = json.loads(report_path.read_bytes())
    validate_detection(report, archive, scanner)
    catalog = json.loads(catalog_path.read_bytes())
    index = catalog_index(catalog, report)
    original = digest_file(archive)
    with tempfile.TemporaryDirectory(prefix="content-identity-", dir=report_path.parent) as temporary:
        identities = archive_identities(archive, Path(temporary), report)
    require(digest_file(archive) == original and valid_digest(review_sha256))
    packet = {"schema": "image-secret-classifications-v1", "archive_sha256": original,
              "scanner_version": report["scanner_version"],
              "gitleaks_rules_sha256": report["policy"]["gitleaks_rules_sha256"],
              "allowlist_sha256": report["policy"]["allowlist_sha256"],
              "review": {"reviewer": "solution-architect-01",
                         "reference": "content-policy-sha256:" + digest_file(catalog_path),
                         "report_sha256": review_sha256},
              "entries": decisions(index, component, report["findings"], identities)}
    packet_bytes = (json.dumps(packet, sort_keys=True) + "\n").encode()
    packet_path = report_path.parent / "content-classifications.json"
    packet_path.write_bytes(packet_bytes)
    assess(report, "credential", packet_path)
    report["exit_code"] = 1 if report["blocking_findings"] else 0
    report_path.write_text(json.dumps(report, sort_keys=True) + "\n")
    return packet_bytes
