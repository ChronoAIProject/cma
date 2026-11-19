"""Validate reviewed, external package evidence for staging image findings."""

import re
from dataclasses import dataclass

from .common import ScanError, archive_path, digest_bytes, json_bytes
from .gate import SELF_TEST_RULES, valid_digest

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


@dataclass(frozen=True)
class PackageEvidence:
    manifest_sha256: str
    review_sha256: str
    scanner_manifest_sha256: str
    packages: dict
    entries: dict

    @classmethod
    def load(cls, data, *, manifest_sha256, review_sha256, scanner_manifest_sha256):
        from . import library_vectors

        require(len(data) <= MAX_EVIDENCE_BYTES and valid_digest(manifest_sha256)
                and valid_digest(review_sha256) and valid_digest(scanner_manifest_sha256)
                and digest_bytes(data) == manifest_sha256)
        raw = json_bytes(data)
        fields(raw, {"schema", "scanner_manifest_sha256", "packages", "entries"})
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
            require(isinstance(row, dict) and row.keys() in (ENTRY_FIELDS, ENTRY_FIELDS | {"self_test"}))
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
            if packages[row["package_sha256"]]["authority_type"] != "pinned-upstream":
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
            self_test = (library_vectors.validate(row, packages[row["package_sha256"]])
                         if "self_test" in row else None)
            verified = {}
            for match in row["matches"]:
                fields(match, MATCH_FIELDS)
                require(valid_digest(match["proof_sha256"])
                        and (match["line"] is None or ordinal(match["line"]) and match["line"] > 0))
                if match["rule"] in PUBLIC_RULES:
                    require(valid_digest(match["candidate_sha256"]) and match["explanation"] in EXPLANATIONS)
                else:
                    library_vectors.acceptable_match(match, self_test)
                identity = (match["rule"], match["candidate_sha256"], match["line"])
                require(identity not in verified)
                verified[identity] = match
            require(self_test is None or any(rule in SELF_TEST_RULES for rule, _, _ in verified))
            entries[key] = (dict(row, path=path), verified)
        return cls(manifest_sha256, review_sha256, scanner_manifest_sha256, packages, entries)
