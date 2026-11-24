"""Classify exact reviewed content after a complete, unchanged detector pass."""
import copy
import hashlib
import json
import os
import re
import tempfile
from pathlib import Path

import image_runtime_headers as runtime_headers
from image_bundle_contexts import entries as bundle_entries
from image_secrets.common import ScanError, archive_path, digest_file
from image_secrets.gate import assess, valid_digest

PUBLIC_RULES = frozenset({"path.key-or-certificate", "gitleaks.image-secret-assignment",
                          "gitleaks.generic-api-key",
                          # Exact rows only: Gitleaks' own default-rule literal in its binary.
                          "gitleaks.aws-amazon-bedrock-api-key-short-lived"})


def published_vector(row):
    # Only an exact reviewed runtime entry can classify a published self-test
    # key. Provenance is bound by the independently approved catalog manifest;
    # it does not replace the file/candidate/rule identity checks below.
    if (row.get("rule") != "gitleaks.private-key" or row.get("component") != "runtime"
            or row.get("scope") != "layer"):
        return False
    proof = row.get("provenance")
    fields = {"package", "package_version", "upstream_tag", "upstream_commit",
              "source_file", "source_line", "source_sha256"}
    if not isinstance(proof, dict) or set(proof) != fields:
        return False
    return (all(isinstance(proof[key], str) and re.fullmatch(r"[A-Za-z0-9.+:~_-]{1,100}", proof[key])
                for key in ("package", "package_version", "upstream_tag"))
            and isinstance(proof["upstream_commit"], str)
            and re.fullmatch(r"[a-f0-9]{40}", proof["upstream_commit"]) is not None
            and isinstance(proof["source_file"], str)
            and re.fullmatch(r"lib/crypto-selftests(?:-pk)?\.c", proof["source_file"]) is not None
            and type(proof["source_line"]) is int and proof["source_line"] > 0
            and valid_digest(proof["source_sha256"]))


def require(condition):
    if not condition:
        raise ScanError("content-policy-evidence-invalid")


# A first-party executable is rebuilt for every publication, so its file digest
# cannot bind a reviewed row. These rows bind the component, its single
# first-party executable, the rule and the exact candidate instead: a candidate
# reviewed as public text stays classified across rebuilds, and any new
# candidate (a changed layout or a new value) still blocks.
FIRST_PARTY_BINARIES = {"am": "usr/local/bin/cma", "eg": "usr/local/bin/cmaeg"}
BINARY_RULES = frozenset({"gitleaks.generic-api-key"})
BINARY_FIELDS = frozenset({"component", "path", "rule", "candidate_sha256", "classification", "reason"})


def binary_entries(catalog):
    rows = catalog.get("binary_candidates", [])
    require(isinstance(rows, list) and len(rows) <= 16)
    observed = set()
    for row in rows:
        require(isinstance(row, dict) and set(row) == BINARY_FIELDS
                and row["component"] in FIRST_PARTY_BINARIES
                and row["path"] == FIRST_PARTY_BINARIES[row["component"]]
                and row["rule"] in BINARY_RULES and valid_digest(row["candidate_sha256"])
                and row["classification"] == "public-material"
                and isinstance(row["reason"], str) and bool(row["reason"].strip()))
        key = ("binary", row["component"], path_hash(row["path"]), row["rule"], row["candidate_sha256"])
        require(key not in observed)
        observed.add(key)
        # A reviewed REAL secret anywhere in the component wins over this row.
        require(not any(exact["classification"] == "REAL" and exact["component"] == row["component"]
                        and exact["rule"] == row["rule"]
                        and exact.get("candidate_sha256") == row["candidate_sha256"]
                        for exact in catalog["entries"]))
        yield key, row


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
    require(catalog.get("schema") in {"cma-image-content-policy-v1", "cma-image-content-policy-v2"})
    require(catalog["schema"] != "cma-image-content-policy-v1"
            or ("bundle_candidates" not in catalog and "binary_candidates" not in catalog))
    for key in ("scanner_version", "gitleaks_version"):
        require(catalog.get(key) == report.get(key))
    for key in ("allowlist_sha256", "gitleaks_rules_sha256"):
        require(catalog.get(key) == report["policy"].get(key))
    index = {}
    for row in catalog["entries"]:
        require(row["component"] in {"web", "am", "eg", "runtime", "runtime-omarchy"}
                and row["scope"] in {"layer", "config"}
                and archive_path(row["path"]) == row["path"] and "*" not in row["path"]
                and valid_digest(row["file_sha256"]) and row["classification"] in {"public-material", "REAL"}
                and isinstance(row["reason"], str) and bool(row["reason"].strip())
                and (row["classification"] == "REAL" or row["rule"] in PUBLIC_RULES or published_vector(row)))
        require("candidate_sha256" not in row or valid_digest(row["candidate_sha256"]))
        require(row["rule"] == "path.key-or-certificate" or "candidate_sha256" in row)
        key = (row["component"], row["scope"], path_hash(row["path"]), row["rule"], row["file_sha256"])
        index.setdefault(key, []).append(row)
    if catalog["schema"] == "cma-image-content-policy-v2":
        for key, row in bundle_entries(catalog):
            index[key] = [row]
        for key, row in binary_entries(catalog):
            index[key] = [row]
    return index


def decisions(index, component, findings, identities, contexts=None, headers=None):
    contexts = {} if contexts is None else contexts
    matched = []
    for finding in findings:
        key = (component, finding["scope"], finding["path"], finding["rule"],
               identities.get(finding["finding_id"]))
        rows = [row for row in index.get(key, [])
                if all(finding.get(field) == row[field] for field in ("candidate_sha256", "line") if field in row)]
        identity = identities.get(finding["finding_id"])
        if (not rows and identity and finding.get("scope") == "layer"
                and finding.get("file_sha256") == identity and finding["finding_id"] in contexts):
            rows = index.get(("bundle", component, contexts[finding["finding_id"]], finding["rule"],
                              finding.get("candidate_sha256")), [])
        if (not rows and identity and finding.get("scope") == "layer"
                and finding.get("file_sha256") == identity):
            rows = index.get(("binary", component, finding["path"], finding["rule"],
                              finding.get("candidate_sha256")), [])
        if (key not in index and headers is not None and finding["finding_id"] in headers.confirmed
                and key == ("runtime", "layer", path_hash(runtime_headers.PATH),
                            runtime_headers.RULE, runtime_headers.FILE_SHA256)):
            rows = [{"classification": "public-material", "reason": runtime_headers.REASON}]
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
    headers = runtime_headers.staging_proof(component, os.environ, catalog["entries"])
    original = digest_file(archive)
    contexts = {}
    with tempfile.TemporaryDirectory(prefix="content-identity-", dir=report_path.parent) as temporary:
        identities = archive_identities(archive, Path(temporary), report, contexts, headers)
    require(digest_file(archive) == original and valid_digest(review_sha256))
    packet = {"schema": "image-secret-classifications-v1", "archive_sha256": original,
              "scanner_version": report["scanner_version"],
              "gitleaks_rules_sha256": report["policy"]["gitleaks_rules_sha256"],
              "allowlist_sha256": report["policy"]["allowlist_sha256"],
              "review": {"reviewer": "solution-architect-01",
                         "reference": "content-policy-sha256:" + digest_file(catalog_path),
                         "report_sha256": review_sha256},
              "entries": decisions(index, component, report["findings"], identities, contexts, headers)}
    packet_bytes = (json.dumps(packet, sort_keys=True) + "\n").encode()
    packet_path = report_path.parent / "content-classifications.json"
    packet_path.write_bytes(packet_bytes)
    assess(report, "credential", packet_path)
    report["exit_code"] = 1 if report["blocking_findings"] else 0
    report_path.write_text(json.dumps(report, sort_keys=True) + "\n")
    return packet_bytes
