"""Bind each reviewed package match to the occurrence actually scanned."""

import json
import tempfile
from pathlib import Path

from .common import digest_bytes, digest_file
from .package_evidence import PUBLIC_RULES, SCOPES, require
from .package_occurrences import collect


def path_digest(path):
    return "path-sha256:" + digest_bytes(path.encode("utf-8", "surrogatepass"))


def exact_match(finding, component, evidence, occurrence):
    platform, config, image, diff_id, (outer_path, outer_sha256, nested) = occurrence
    member_id = finding.get("member")
    selected = evidence.entries.get((component, platform, finding["layer"], diff_id, finding["entry"], member_id))
    if selected is None:
        return None
    expected, reviews = selected
    if outer_path != expected["path"] or outer_sha256 != expected["file_sha256"]:
        return None
    path, observed_sha256 = outer_path, outer_sha256
    if member_id is not None:
        candidate_member = nested.get(member_id)
        member = expected["member"]
        if candidate_member != ("!".join([outer_path, *member["path"]]), member["sha256"]):
            return None
        path, observed_sha256 = candidate_member
    if finding["path"] != path_digest(path) or finding.get("file_sha256") != observed_sha256:
        return None
    review = reviews.get((finding["rule"], finding.get("candidate_sha256"), finding.get("line")))
    if review is None:
        return None
    return {
        "finding_id": finding["finding_id"],
        "rule": finding["rule"],
        "classification": "public-material",
        "platform": platform,
        "image_manifest": image,
        "image_config": config,
        "image": finding["image"],
        "layer": finding["layer"],
        "layer_digest": diff_id,
        "entry": finding["entry"],
        "member": member_id,
        "path_digest": finding["path"],
        "file_sha256": observed_sha256,
        "outer_path_digest": path_digest(outer_path),
        "outer_file_sha256": outer_sha256,
        "candidate_sha256": finding["candidate_sha256"],
        "line": finding.get("line"),
        "package": evidence.packages[expected["package_sha256"]],
        "package_member": {
            "locator": expected["package_member"]["locator"],
            "path_digest": path_digest("!".join(expected["package_member"]["path"])),
            "type": expected["package_member"]["type"],
            "sha256": expected["package_member"]["sha256"],
        },
        "explanation": review["explanation"],
        "proof_sha256": review["proof_sha256"],
    }


def build(archive, output, report, component, evidence):
    require(component in SCOPES and report["policy"].get("staging_runtime_component") == component)
    require(report["archive_sha256"] == digest_file(archive))
    with tempfile.TemporaryDirectory(prefix="package-occurrences-", dir=output) as tmp:
        observations = collect(archive, Path(tmp), report, component, evidence)
    rows = []
    for finding in report["findings"]:
        if finding.get("scope") != "layer" or finding.get("rule") not in PUBLIC_RULES:
            continue
        occurrence = observations.get((finding["image"], finding["layer"], finding["entry"]))
        if occurrence is not None:
            result = exact_match(finding, component, evidence, occurrence)
            if result is not None:
                rows.append(result)
    require(report["archive_sha256"] == digest_file(archive))
    return {
        "schema": "cma-staging-package-decisions-v1",
        "scope": "staging",
        "component": component,
        "archive_sha256": report["archive_sha256"],
        "scanner_version": report["scanner_version"],
        "scanner_manifest_sha256": evidence.scanner_manifest_sha256,
        "evidence_manifest_sha256": evidence.manifest_sha256,
        "review_sha256": evidence.review_sha256,
        "entries": rows,
    }


def digest(section):
    return digest_bytes(json.dumps(section, sort_keys=True, separators=(",", ":")).encode())


def decisions(section):
    rows = section["entries"]
    result = {row["finding_id"]: row["classification"] for row in rows}
    require(len(result) == len(rows))
    return result
