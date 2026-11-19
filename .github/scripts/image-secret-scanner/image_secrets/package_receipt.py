"""Bind each reviewed package match to the occurrence actually scanned."""

import json
import tempfile
from pathlib import Path

from . import arch_trust, nested_certificates
from .common import digest_bytes, digest_file
from .gate import (
    ADVISORY_RULES,
    ARCH_CERTIFICATE_ADVISORY,
    CERTIFICATE_ADVISORY,
    CERTIFICATE_RULE,
    DIGEST_FRAME_ADVISORY,
    EXECUTABLE_TEST_ADVISORY,
    SELF_TEST_RULES,
    TEST_VECTOR_ADVISORIES,
)
from .package_evidence import PUBLIC_RULES, SCOPES, require
from .package_occurrences import collect


def path_digest(path):
    return "path-sha256:" + digest_bytes(path.encode("utf-8", "surrogatepass"))


def receipt_context(finding, occurrence, observed_sha256):
    platform, config, image, diff_id, (outer_path, outer_sha256, _) = occurrence
    return {
        "finding_id": finding["finding_id"],
        "rule": finding["rule"],
        "platform": platform,
        "image_manifest": image,
        "image_config": config,
        "image": finding["image"],
        "layer": finding["layer"],
        "layer_digest": diff_id,
        "entry": finding["entry"],
        "member": finding.get("member"),
        "path_digest": finding["path"],
        "file_sha256": observed_sha256,
        "outer_path_digest": path_digest(outer_path),
        "outer_file_sha256": outer_sha256,
        "candidate_sha256": finding.get("candidate_sha256"),
        "line": finding.get("line"),
    }


def checked_vectors(finding, expected, verified, database_checks):
    proof = expected["self_test"]
    db = proof["database"]
    coordinate = (finding["image"], db["layer"], db["entry"])
    database_identity = (db["diff_id"], db["path"], db["sha256"], expected["path"], db["member_checksum"])
    if database_identity not in database_checks.get(coordinate, ()):
        return None
    spans = verified.get(finding["rule"], [])
    if finding["rule"] == "gitleaks.private-key":
        spans = [span for span in spans if span["sha256"] == finding["candidate_sha256"]]
    if not spans:
        return None
    path_key = "executable_path" if "executable" in proof else "library_path"
    return {path_key: expected["package_member"]["path"][-1], "database": db,
            "upstream": proof["upstream"], "vectors": spans}


def certificate_evidence(report, finding, expected, checked, config, image):
    if checked is None:
        return None
    proof = expected["nested_certificate"]
    if (proof["archive_sha256"] != report["archive_sha256"] or proof["image_config"] != config
            or proof["image_manifest"] != image or proof["finding_id"] != finding["finding_id"]
            or proof["der_sha256s"] != checked["der_sha256s"]):
        return None
    coordinates = ("image", "layer", "entry", "member")
    locator = {key: finding[key] for key in coordinates}
    observed = dict(locator, **checked)
    if observed not in report["inventory"].get("nested_certificates", ()):
        return None
    if any(item["finding_id"] != finding["finding_id"] and item["rule"] not in ADVISORY_RULES
           and all(item.get(key) == locator[key] for key in coordinates)
           for item in report["findings"]):
        return None
    return {"pypi_attestation_sha256": proof["pypi_attestation_sha256"],
            "wheel_path": nested_certificates.WHEEL, "member_path": nested_certificates.MEMBER, **checked}


def exact_match(report, finding, component, evidence, occurrence, database_checks, verified):
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
    review = reviews.get((finding["rule"], finding.get("candidate_sha256"), finding.get("line")))
    if review is None:
        return None
    explanation = review["explanation"]
    raw_checksum = finding.get("file_sha256")
    aggregate = (explanation in TEST_VECTOR_ADVISORIES | {DIGEST_FRAME_ADVISORY}
                 and finding["rule"] != "gitleaks.private-key")
    if finding["path"] != path_digest(path) or (raw_checksum != observed_sha256 and not (aggregate and raw_checksum is None)):
        return None
    classification = "public-material"
    extra = {}
    if explanation in TEST_VECTOR_ADVISORIES:
        if explanation == EXECUTABLE_TEST_ADVISORY:
            binding = expected["self_test"]["executable"]
            if (binding["archive_sha256"] != report["archive_sha256"] or binding["image_config"] != config
                    or binding["image_manifest"] != image or finding["finding_id"] not in binding["finding_ids"]):
                return None
        vector_proof = checked_vectors(finding, expected, verified, database_checks)
        if vector_proof is None:
            return None
        classification = explanation
        proof_key = "executable_self_test" if explanation == EXECUTABLE_TEST_ADVISORY else "library_self_test"
        extra[proof_key] = vector_proof
    elif explanation == DIGEST_FRAME_ADVISORY:
        frame_proof = verified.get(DIGEST_FRAME_ADVISORY)
        if frame_proof is None:
            return None
        classification = explanation
        extra["public_digest_frames"] = frame_proof
    elif explanation == CERTIFICATE_ADVISORY:
        certificate = certificate_evidence(report, finding, expected, verified.get(CERTIFICATE_ADVISORY),
                                           config, image)
        if certificate is None:
            return None
        classification = explanation
        extra["nested_certificate"] = certificate
    result = {
        **receipt_context(finding, occurrence, observed_sha256),
        "classification": classification,
        "package": evidence.packages[expected["package_sha256"]],
        "package_member": {
            "locator": expected["package_member"]["locator"],
            "path_digest": path_digest("!".join(expected["package_member"]["path"])),
            "type": expected["package_member"]["type"],
            "sha256": expected["package_member"]["sha256"],
        },
        "explanation": explanation,
        "proof_sha256": review["proof_sha256"],
        **extra,
    }
    return result


def arch_trust_decisions(report, evidence, observations, verified):
    packet = evidence.arch_trust
    raw = {row["finding_id"]: row for row in report["findings"]}
    seen = report["inventory"].get("arch_trust", {})
    coordinate_keys = ("image", "layer", "entry")
    unresolved = {tuple(row[key] for key in coordinate_keys) for row in raw.values()
                  if row["scope"] == "layer" and row["rule"] not in ADVISORY_RULES
                  and (row["rule"] != CERTIFICATE_RULE or "member" in row)}
    rows = []
    target = None
    pending = list(packet["files"].values())
    if packet["alias"] is not None:
        pending.append(packet["alias"])
    for expected in pending:
        finding = raw.get(expected["finding_id"])
        if (finding is None or finding["scope"] != "layer" or finding["rule"] != CERTIFICATE_RULE
                or "member" in finding or finding["layer"] != arch_trust.LAYER
                or finding["entry"] != expected["entry"] or finding["path"] != path_digest(expected["path"])):
            continue
        coordinate = tuple(finding[key] for key in coordinate_keys)
        occurrence = observations.get(coordinate)
        checked = verified.get(coordinate, {}).get(ARCH_CERTIFICATE_ADVISORY)
        if coordinate in unresolved or occurrence is None or checked is None:
            continue
        platform, _, _, diff_id, (path, file_sha256, _) = occurrence
        if (platform != arch_trust.PLATFORM or diff_id != packet["base"]["layer_diff_id"]
                or path != expected["path"] or file_sha256 != finding.get("file_sha256")
                or file_sha256 != finding.get("candidate_sha256")):
            continue
        position = dict(zip(coordinate_keys, coordinate))
        proof = {"public_path": path, "base": packet["base"], **checked}
        row = {**receipt_context(finding, occurrence, file_sha256), "classification": ARCH_CERTIFICATE_ADVISORY,
               "arch_trust": proof, "proof_sha256": expected["proof_sha256"]}
        if path == arch_trust.ALIAS:
            if (target is None or target["image"] != row["image"] or position not in seen.get("aliases", ())
                    or checked["link_target"] != expected["link_target"] or checked.get("namespace_verified") is not True):
                continue
            proof["target_finding_id"] = target["finding_id"]
        else:
            if (dict(position, **checked) not in seen.get("files", ()) or file_sha256 != expected["file_sha256"]
                    or checked["der_sha256s"] != expected["der_sha256s"]):
                continue
            native = expected["native"]
            if native is not None:
                proof["native"] = dict(native, package=evidence.packages[native["package_sha256"]])
            if path == arch_trust.TLS:
                target = row
        rows.append(row)
    return rows


def build(archive, output, report, component, evidence):
    require(component in SCOPES and report["policy"].get("staging_runtime_component") == component)
    require(report["archive_sha256"] == digest_file(archive))
    vector_checks = {}
    database_checks = {}
    with tempfile.TemporaryDirectory(prefix="package-occurrences-", dir=output) as tmp:
        observations = collect(archive, Path(tmp), report, component, evidence,
                               vector_checks=vector_checks, database_checks=database_checks)
    rows = []
    for finding in report["findings"]:
        if finding.get("scope") != "layer" or finding.get("rule") not in PUBLIC_RULES | SELF_TEST_RULES | {CERTIFICATE_RULE}:
            continue
        coordinate = (finding["image"], finding["layer"], finding["entry"])
        occurrence = observations.get(coordinate)
        if occurrence is not None:
            result = exact_match(report, finding, component, evidence, occurrence, database_checks,
                                 vector_checks.get(coordinate, {}))
            if result is not None:
                rows.append(result)
    if component == arch_trust.COMPONENT and evidence.arch_trust is not None:
        rows.extend(arch_trust_decisions(report, evidence, observations, vector_checks))
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
