"""Bind each reviewed package match to the occurrence actually scanned."""

import json
import tempfile
from pathlib import Path

from . import arch_trust, nested_certificates, npm_help_template, reviewed_literals
from .common import digest_bytes, digest_file
from .gate import (
    ADVISORY_RULES,
    ARCH_CERTIFICATE_ADVISORY,
    CERTIFICATE_ADVISORY,
    CERTIFICATE_RULE,
    DEBIAN_CONTROL_ADVISORY,
    DIGEST_FRAME_ADVISORY,
    DISTRIBUTION_ADVISORY,
    DISTRIBUTION_RULE,
    EXECUTABLE_TEST_ADVISORY,
    GO_CLI_ADVISORY,
    GO_CLI_RULES,
    KEY_MARKER_VIEW_ADVISORY,
    SELF_TEST_RULES,
    TEST_VECTOR_ADVISORIES,
)
from .package_evidence import PUBLIC_RULES, SCOPES, require
from .package_occurrences import collect
from .portable_occurrences import PortableOccurrences

PACKAGE_MATCH_RULES = PUBLIC_RULES | SELF_TEST_RULES | reviewed_literals.RULES | GO_CLI_RULES | {
    CERTIFICATE_RULE, DISTRIBUTION_RULE,
}


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


def checked_database(finding, expected, db, database_checks):
    coordinate = (finding["image"], db["layer"], db["entry"])
    database_identity = (db["diff_id"], db["path"], db["sha256"], expected["path"], db["member_checksum"])
    return database_identity in database_checks.get(coordinate, ())


def portable_database(finding, expected, db, database_checks, diff_id):
    identity = (None, db["path"], db["sha256"], expected["path"], db["member_checksum"])
    entries = [entry for (image, layer, entry), checks in database_checks.items()
               if (image, layer) == (finding["image"], finding["layer"]) and identity in checks]
    if len(entries) != 1:
        return None
    return dict(db, layer=finding["layer"], diff_id=diff_id, entry=entries[0])


def occurrence_database(finding, expected, db, database_checks, *, portable, diff_id):
    if portable:
        return portable_database(finding, expected, db, database_checks, diff_id)
    return db if checked_database(finding, expected, db, database_checks) else None


def checked_vectors(finding, expected, verified, database_checks, *, portable, diff_id):
    proof = expected["self_test"]
    db = occurrence_database(finding, expected, proof["database"], database_checks,
                             portable=portable, diff_id=diff_id)
    if db is None:
        return None
    spans = verified.get(finding["rule"], [])
    if finding["rule"] == "gitleaks.private-key":
        spans = [span for span in spans if span["sha256"] == finding["candidate_sha256"]]
    if not spans:
        return None
    path_key = "executable_path" if "executable" in proof else "library_path"
    return {path_key: expected["package_member"]["path"][-1], "database": db,
            "upstream": proof["upstream"], "vectors": spans}


def matches_image(proof, report, config, image):
    return (proof["archive_sha256"] == report["archive_sha256"] and proof["image_config"] == config
            and proof["image_manifest"] == image)


def certificate_evidence(report, finding, expected, checked, config, image, *, portable=False):
    if checked is None:
        return None
    proof = expected["nested_certificate"]
    if (not portable and (not matches_image(proof, report, config, image) or proof["finding_id"] != finding["finding_id"])
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


def exact_match(report, finding, component, evidence, occurrence, database_checks, verified, *, portable_row=None):
    if finding.get("view") == "pax-xattr":
        return None
    platform, config, image, diff_id, (outer_path, outer_sha256, nested) = occurrence
    member_id = finding.get("member")
    portable = portable_row is not None
    selected = portable_row[:2] if portable else evidence.entries.get(
        (component, platform, finding["layer"], diff_id, finding["entry"], member_id))
    if selected is None:
        return None
    expected, reviews = selected
    if outer_path != expected["path"] or outer_sha256 != expected["file_sha256"]:
        return None
    path, observed_sha256 = outer_path, outer_sha256
    member = expected["member"]
    if (member is None) != (member_id is None):
        return None
    if member_id is not None:
        # The reopened member must be the regular member the detector decoded at this locator.
        candidate_member = nested.get(member_id)
        if member["locator"] != member_id or candidate_member != ("!".join([outer_path, *member["path"]]),
                                                                  member["sha256"]):
            return None
        path, observed_sha256 = candidate_member
    review = reviews.get((finding["rule"], finding.get("candidate_sha256"), finding.get("line")))
    if review is None:
        return None
    explanation = review["explanation"]
    classification = "public-material"
    extra = {"current_owner": portable_row[2]} if portable else {}
    finding_id = finding["finding_id"]
    image_identity = {"archive_sha256": report["archive_sha256"], "image_config": config, "image_manifest": image}
    origin = expected.get("derived_payload")
    if origin is not None:
        physically_verified = (verified.get("derived_payload") if member_id is None
                               else member_id in verified.get("nested_derived", ()))
        if (not physically_verified or not portable and (not matches_image(origin, report, config, image)
                                                         or finding_id not in origin["findings"])):
            return None
        extra["derived_payload"] = dict(origin, **image_identity, findings=[finding_id]) if portable else origin
    if explanation == DEBIAN_CONTROL_ADVISORY:
        control = verified.get(DEBIAN_CONTROL_ADVISORY)
        if (control is None or not portable and (not matches_image(control, report, config, image)
                                                or control["finding_id"] != finding_id)):
            return None
        classification = explanation
        observed_sha256 = control["view_sha256"]
        extra["debian_control"] = {"installed_path": path, **control, **image_identity, "finding_id": finding_id}
    if explanation in reviewed_literals.CLASSES:
        checks = (verified.get("reviewed_literals", {}) if member_id is None
                  else verified.get("nested_literals", {}).get(member_id, {}))
        claim = checks.get(finding["rule"])
        binding = expected["reviewed_literals"]
        if (claim is None or not portable and (claim["finding_id"] != finding_id
                                              or not matches_image(binding, report, config, image))):
            return None
        classification = explanation
        extra["reviewed_literal"] = {**claim, **image_identity, "finding_id": finding_id}
        if finding["rule"] == npm_help_template.RULE:
            for key in ("npm_help_template", "nyxid_format_labels"):
                if key in expected:
                    extra[key] = expected[key]
    if explanation == KEY_MARKER_VIEW_ADVISORY:
        candidate = finding.get("candidate_sha256")
        for key in ("key_marker_view", "native_json_key_view", "elf_nul_key_view"):
            claim = verified.get(key, {}).get(candidate)
            if claim is not None:
                break
        if claim is None or not portable and claim["finding_id"] != finding_id:
            return None
        proof = expected[key]
        if not portable and not matches_image(proof, report, config, image):
            return None
        classification = explanation
        extra[key] = {**claim, **{field: value for field, value in proof.items() if field != "matches"},
                      **image_identity, "finding_id": finding_id}
    if explanation == GO_CLI_ADVISORY:
        claim = verified.get("go_cli_strings", {}).get((finding["rule"], finding.get("candidate_sha256")))
        if claim is None or not portable and claim["finding_id"] != finding_id:
            return None
        classification = explanation
        extra["derived_payload"] = dict(origin, **image_identity, strings=[dict(claim, finding_id=finding_id)],
                                        findings=[finding_id])
    raw_checksum = finding.get("file_sha256")
    aggregate = (explanation in TEST_VECTOR_ADVISORIES | reviewed_literals.CLASSES | {DIGEST_FRAME_ADVISORY}
                 and finding["rule"] != "gitleaks.private-key")
    if finding["path"] != path_digest(path) or (raw_checksum != observed_sha256 and not (aggregate and raw_checksum is None)):
        return None
    if explanation in TEST_VECTOR_ADVISORIES:
        if explanation == EXECUTABLE_TEST_ADVISORY and not portable:
            binding = expected["self_test"]["executable"]
            if (not matches_image(binding, report, config, image)
                    or finding["finding_id"] not in binding["finding_ids"]):
                return None
        vector_proof = checked_vectors(finding, expected, verified, database_checks,
                                       portable=portable, diff_id=diff_id)
        if vector_proof is None:
            return None
        classification = explanation
        proof_key = "executable_self_test" if explanation == EXECUTABLE_TEST_ADVISORY else "library_self_test"
        extra[proof_key] = vector_proof
    elif explanation == DISTRIBUTION_ADVISORY:
        distribution = expected["distribution_config"]
        if (not verified.get(DISTRIBUTION_ADVISORY) or not portable
                and (not matches_image(distribution, report, config, image) or distribution["finding_id"] != finding_id)):
            return None
        db = occurrence_database(finding, expected, distribution["database"], database_checks,
                                 portable=portable, diff_id=diff_id)
        if db is None:
            return None
        classification = explanation
        extra["distribution_config"] = {**distribution, **image_identity, "public_path": outer_path,
                                        "finding_id": finding_id, "database": db}
    elif explanation == DIGEST_FRAME_ADVISORY:
        frame_proof = verified.get(DIGEST_FRAME_ADVISORY)
        if frame_proof is None:
            return None
        classification = explanation
        extra["public_digest_frames"] = frame_proof
    elif explanation == CERTIFICATE_ADVISORY:
        certificate = certificate_evidence(report, finding, expected, verified.get(CERTIFICATE_ADVISORY),
                                           config, image, portable=portable)
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


def arch_trust_decisions(report, evidence, observations, verified, packet):
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
        platform, config, manifest, diff_id, (path, file_sha256, _) = occurrence
        if (platform != arch_trust.PLATFORM or diff_id != arch_trust.physical_layer(packet)
                or not arch_trust.matches_image(packet, report["archive_sha256"], config, manifest)
                or path != expected["path"] or file_sha256 != finding.get("file_sha256")
                or file_sha256 != finding.get("candidate_sha256")):
            continue
        position = dict(zip(coordinate_keys, coordinate))
        proof = {"public_path": path, "base": packet["base"], **checked}
        if "repack" in packet:
            proof["repack"] = packet["repack"]
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


def portable_trust_decisions(report, evidence, observations, verified, packet, owners):
    """Locate the reviewed paths by name; every image, layer, entry and finding identity is this scan's."""
    seen = report["inventory"].get("arch_trust", {})
    coordinate_keys = ("image", "layer", "entry")
    unresolved = {tuple(row[key] for key in coordinate_keys) for row in report["findings"]
                  if row["scope"] == "layer" and row["rule"] not in ADVISORY_RULES
                  and (row["rule"] != CERTIFICATE_RULE or "member" in row)}
    expected_paths = {path_digest(path): expected for path, expected in arch_trust.portable_entries(packet).items()}
    candidates = [row for row in report["findings"] if row["scope"] == "layer" and row["rule"] == CERTIFICATE_RULE
                  and "member" not in row and row["layer"] == arch_trust.LAYER and row["path"] in expected_paths]
    rows = []
    targets = {}
    # The alias is decided only after its image's TLS bundle has its own decision.
    for finding in sorted(candidates, key=lambda row: row["path"] == path_digest(arch_trust.ALIAS)):
        expected = expected_paths[finding["path"]]
        coordinate = tuple(finding[key] for key in coordinate_keys)
        occurrence = observations.get(coordinate)
        checked = verified.get(coordinate, {}).get(ARCH_CERTIFICATE_ADVISORY)
        if (coordinate in unresolved or occurrence is None or checked is None
                or checked.get("namespace_verified") is not True):
            continue
        platform, _, _, _, (path, file_sha256, _) = occurrence
        if (platform != arch_trust.PLATFORM or path != expected["path"] or file_sha256 != finding.get("file_sha256")
                or file_sha256 != finding.get("candidate_sha256")):
            continue
        position = dict(zip(coordinate_keys, coordinate))
        proof = {"public_path": path, "catalog": "portable", **checked}
        row = {**receipt_context(finding, occurrence, file_sha256), "classification": ARCH_CERTIFICATE_ADVISORY,
               "arch_trust": proof, "proof_sha256": expected["proof_sha256"]}
        if path == arch_trust.ALIAS:
            target = targets.get(finding["image"])
            if (target is None or position not in seen.get("aliases", ())
                    or checked["link_target"] != expected["link_target"]):
                continue
            proof["target_finding_id"] = target["finding_id"]
        else:
            observed = dict(position, member_sha256=checked["member_sha256"], der_sha256s=checked["der_sha256s"])
            if (observed not in seen.get("files", ()) or file_sha256 != expected["file_sha256"]
                    or checked["der_sha256s"] != expected["der_sha256s"]):
                continue
            native = expected["native"]
            if native is not None:
                owner = owners.get(coordinate)
                if owner is None:
                    continue
                proof["native"] = dict(native, package=evidence.packages[native["package_sha256"]], current_owner=owner)
            if path == arch_trust.TLS:
                targets[finding["image"]] = row
        rows.append(row)
    return rows


def build(archive, output, report, component, evidence, *, source=None):
    require(component in SCOPES and report["policy"].get("staging_runtime_component") == component)
    require(report["archive_sha256"] == digest_file(archive))
    vector_checks = {}
    database_checks = {}
    trust = evidence.portable_trust if component == evidence.portable_component else None
    portable = (PortableOccurrences(evidence.portable_entries or {}, evidence.packages, source, component, trust=trust)
                if component == evidence.portable_component and (evidence.portable_entries or trust) else None)
    with tempfile.TemporaryDirectory(prefix="package-occurrences-", dir=output) as tmp:
        observations = collect(archive, Path(tmp), report, component, evidence,
                               vector_checks=vector_checks, database_checks=database_checks, portable=portable)
    portable_rows = portable.eligible(observations) if portable is not None else {}
    rows = []
    for finding in report["findings"]:
        if finding.get("scope") != "layer" or finding.get("rule") not in PACKAGE_MATCH_RULES:
            continue
        coordinate = (finding["image"], finding["layer"], finding["entry"])
        occurrence = observations.get(coordinate)
        if occurrence is not None:
            reviewed = portable_rows.get(coordinate, {}).get(finding.get("member"))
            result = exact_match(report, finding, component, evidence, occurrence, database_checks,
                                 vector_checks.get(coordinate, {}), portable_row=reviewed)
            if result is not None:
                rows.append(result)
    if component == arch_trust.COMPONENT and evidence.arch_trust:
        for packet in evidence.arch_trust.values():
            rows.extend(arch_trust_decisions(report, evidence, observations, vector_checks, packet))
    if component == arch_trust.COMPONENT and trust is not None:
        rows.extend(portable_trust_decisions(report, evidence, observations, vector_checks, trust,
                                             portable.trust_owners(observations)))
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
