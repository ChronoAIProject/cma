"""Validate reviewed, external package evidence for staging image findings."""

import re
from dataclasses import dataclass

from . import nested_certificates
from .common import ScanError, archive_path, digest_bytes, json_bytes
from .gate import (
    CERTIFICATE_ADVISORY,
    CERTIFICATE_RULE,
    DIGEST_FRAME_ADVISORY,
    DIGEST_FRAME_RULE,
    DISTRIBUTION_RULE,
    SELF_TEST_RULES,
    valid_digest,
)

SCOPES = frozenset({"runtime", "runtime-omarchy"})
AUTHORITY_TYPES = frozenset({"debian-index", "arch-signature", "apk-index", "pinned-upstream"})
# Evidence for public text does not authorize file-wide or private-key decisions.
PUBLIC_RULES = frozenset({"gitleaks.image-secret-assignment", "gitleaks.image-hex-secret-assignment",
                          "gitleaks.generic-api-key"})
EXPLANATIONS = frozenset({"public-parser-delimiter", "caller-supplied-value-expression",
                          "public-noncredential-constant", "object-key-count-expression"})
PACKAGE_FIELDS = frozenset({"name", "version", "architecture", "artifact_sha256",
                            "authority_type", "authority_sha256"})
MATCH_FIELDS = frozenset({"rule", "candidate_sha256", "line", "explanation", "proof_sha256"})
ENTRY_FIELDS = frozenset({"component", "platform", "layer", "diff_id", "entry", "path",
                         "file_sha256", "package_sha256", "package_member", "member", "matches"})
MAX_EVIDENCE_BYTES = 16 * 1024 * 1024
IDENTIFIER = re.compile(r"[A-Za-z0-9][A-Za-z0-9.+:~_-]{0,199}")
PLATFORM = re.compile(r"[a-z0-9]+/[a-z0-9_]+(?:/[a-z0-9]+)?")
LAYER_ID = re.compile(r"sha256:[a-f0-9]{64}")
NESTED_LOCATOR = re.compile(r"(?:0|[1-9][0-9]{0,6})(?:/(?:0|[1-9][0-9]{0,6})){0,2}")


def require(condition):
    if not condition:
        raise ScanError("package-evidence-invalid")


def fields(value, expected):
    require(isinstance(value, dict) and value.keys() == expected)


def matches(pattern, value):
    return isinstance(value, str) and pattern.fullmatch(value) is not None


def normalized(path):
    require(isinstance(path, str) and 0 < len(path) <= 4096 and path != "."
            and all(ord(char) >= 32 for char in path) and "!" not in path)
    require(archive_path(path) == path)
    return path


def ordinal(value):
    return type(value) is int and 0 <= value <= 1_000_000


def validate_nested_certificate(row, package):
    proof = row["nested_certificate"]
    member = row["member"]
    fields(proof, {"pypi_attestation_sha256", "archive_sha256", "image_config", "image_manifest",
                   "finding_id", "der_sha256s"})
    require(row["component"] == nested_certificates.COMPONENT and row["path"] == nested_certificates.WHEEL
            and member is not None and member["path"] == [nested_certificates.MEMBER]
            and package["authority_type"] == "arch-signature" and package["name"] == "python"
            and all(valid_digest(proof[key]) for key in ("pypi_attestation_sha256", "archive_sha256", "finding_id"))
            and all(matches(LAYER_ID, proof[key]) for key in ("image_config", "image_manifest")))
    hashes = proof["der_sha256s"]
    require(isinstance(hashes, list) and len(hashes) == nested_certificates.COUNT
            and all(valid_digest(value) for value in hashes) and hashes == sorted(set(hashes)))
    require(len(row["matches"]) == 1)
    match = row["matches"][0]
    fields(match, MATCH_FIELDS)
    require(match["rule"] == CERTIFICATE_RULE and match["explanation"] == CERTIFICATE_ADVISORY
            and match["candidate_sha256"] == member["sha256"] and match["line"] is None
            and valid_digest(match["proof_sha256"]))
    return {(CERTIFICATE_RULE, member["sha256"], None): match}


@dataclass(frozen=True)
class PackageEvidence:
    manifest_sha256: str
    review_sha256: str
    scanner_manifest_sha256: str
    packages: dict
    entries: dict
    arch_trust: dict = None

    @classmethod
    def load(cls, data, *, manifest_sha256, review_sha256, scanner_manifest_sha256):
        from . import arch_trust_evidence, debian_control, digest_frames, distribution_config, library_vectors

        require(len(data) <= MAX_EVIDENCE_BYTES and valid_digest(manifest_sha256)
                and valid_digest(review_sha256) and valid_digest(scanner_manifest_sha256)
                and digest_bytes(data) == manifest_sha256)
        raw = json_bytes(data)
        expected_fields = {"schema", "scanner_manifest_sha256", "packages", "entries"}
        require(isinstance(raw, dict) and raw.keys() in (expected_fields, expected_fields | {"arch_trust"}))
        require(raw["schema"] == "cma-staging-package-evidence-v1"
                and raw["scanner_manifest_sha256"] == scanner_manifest_sha256
                and isinstance(raw["packages"], list) and isinstance(raw["entries"], list))
        packages = {}
        for package in raw["packages"]:
            fields(package, PACKAGE_FIELDS)
            artifact = package["artifact_sha256"]
            require(all(matches(IDENTIFIER, package[key]) for key in ("name", "version", "architecture"))
                    and package["authority_type"] in AUTHORITY_TYPES
                    and valid_digest(artifact) and valid_digest(package["authority_sha256"])
                    and artifact not in packages)
            packages[artifact] = package

        entries = {}
        for row in raw["entries"]:
            require(isinstance(row, dict) and row.keys() in (ENTRY_FIELDS, ENTRY_FIELDS | {"self_test"},
                                                           ENTRY_FIELDS | {"digest_frames"},
                                                           ENTRY_FIELDS | {"nested_certificate"},
                                                           ENTRY_FIELDS | {"distribution_config"},
                                                           ENTRY_FIELDS | {"debian_control"},
                                                           ENTRY_FIELDS | {"distribution_config", "self_test"}))
            require(row["component"] in SCOPES and matches(PLATFORM, row["platform"])
                    and matches(LAYER_ID, row["diff_id"]) and ordinal(row["layer"])
                    and ordinal(row["entry"]) and valid_digest(row["file_sha256"])
                    and valid_digest(row["package_sha256"]) and row["package_sha256"] in packages
                    and isinstance(row["matches"], list) and bool(row["matches"]))
            path = normalized(row["path"])
            package_member = row["package_member"]
            fields(package_member, {"locator", "path", "type", "sha256"})
            require(matches(NESTED_LOCATOR, package_member["locator"])
                    and package_member["type"] == "regular" and package_member["sha256"] == row["file_sha256"]
                    and isinstance(package_member["path"], list)
                    and len(package_member["path"]) == package_member["locator"].count("/") + 1)
            for part in package_member["path"]:
                normalized(part)
            package = packages[row["package_sha256"]]
            if package["authority_type"] != "pinned-upstream" and "debian_control" not in row:
                require(package_member["path"][-1] == path)
            member = row["member"]
            locator = None
            if member is not None:
                fields(member, {"locator", "path", "sha256"})
                locator = member["locator"]
                require(matches(NESTED_LOCATOR, locator) and valid_digest(member["sha256"])
                        and isinstance(member["path"], list) and len(member["path"]) == locator.count("/") + 1)
                for part in member["path"]:
                    normalized(part)
            key = (row["component"], row["platform"], row["layer"], row["diff_id"], row["entry"], locator)
            require(key not in entries)
            if "nested_certificate" in row:
                entries[key] = (row, validate_nested_certificate(row, package))
                continue
            if "debian_control" in row:
                entries[key] = (row, debian_control.validate(row, package))
                continue
            self_test = library_vectors.validate(row, package) if "self_test" in row else None
            distribution = distribution_config.validate(row, package) if "distribution_config" in row else None
            if distribution is not None and self_test is not None:
                require("executable" in self_test)
            public_frames = "digest_frames" in row
            if public_frames:
                digest_frames.validate(row)
            verified = {}
            for match in row["matches"]:
                fields(match, MATCH_FIELDS)
                require(valid_digest(match["proof_sha256"])
                        and (match["line"] is None or ordinal(match["line"]) and match["line"] > 0))
                if match["rule"] in PUBLIC_RULES:
                    require(valid_digest(match["candidate_sha256"]) and match["explanation"] in EXPLANATIONS)
                elif match["rule"] == DISTRIBUTION_RULE:
                    distribution_config.acceptable_match(match, distribution)
                elif public_frames:
                    digest_frames.acceptable_match(match)
                else:
                    library_vectors.acceptable_match(match, self_test)
                identity = (match["rule"], match["candidate_sha256"], match["line"])
                require(identity not in verified)
                verified[identity] = match
            if self_test is not None:
                vector_advisory = library_vectors.classification(self_test)
                vector_matches = [match for (rule, _, _), match in verified.items()
                                  if rule in SELF_TEST_RULES and match["explanation"] == vector_advisory]
                require(bool(vector_matches))
                if "executable" in self_test:
                    require(len(vector_matches) == len(self_test["executable"]["finding_ids"]))
            if public_frames:
                match = verified.get((DIGEST_FRAME_RULE, None, None))
                require(match is not None and match["explanation"] == DIGEST_FRAME_ADVISORY)
            if distribution is not None:
                require((DISTRIBUTION_RULE, distribution["span"]["sha256"], None) in verified)
            entries[key] = (dict(row, path=path), verified)
        arch = arch_trust_evidence.load(raw["arch_trust"], packages) if "arch_trust" in raw else None
        return cls(manifest_sha256, review_sha256, scanner_manifest_sha256, packages, entries, arch)
