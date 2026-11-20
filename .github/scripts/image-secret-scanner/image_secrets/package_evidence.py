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
    KEY_MARKER_VIEW_ADVISORY,
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
PORTABLE_SCHEMA = "cma-staging-runtime-package-evidence-v2"
OMARCHY_PORTABLE_SCHEMA = "cma-staging-omarchy-package-evidence-v2"
PORTABLE_SCOPES = {PORTABLE_SCHEMA: "runtime", OMARCHY_PORTABLE_SCHEMA: "runtime-omarchy"}
PORTABLE_ENTRY_FIELDS = ENTRY_FIELDS - {"layer", "diff_id", "entry"} | {"occurrences", "ownership"}
IMAGE_FIELDS = frozenset({"archive_sha256", "image_config", "image_manifest"})
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


def image_binding(proof, fields_without_binding, *, portable, finding_fields=()):
    if portable:
        fields(proof, fields_without_binding)
    else:
        fields(proof, fields_without_binding | IMAGE_FIELDS | set(finding_fields))
        require(valid_digest(proof["archive_sha256"]) and matches(LAYER_ID, proof["image_config"])
                and (proof["image_manifest"] is None or matches(LAYER_ID, proof["image_manifest"])))


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
    portable_entries: dict = None
    portable_component: str = None

    @classmethod
    def load(cls, data, *, manifest_sha256, review_sha256, scanner_manifest_sha256):
        from . import (
            arch_trust_evidence,
            certificate_blocks,
            debian_control,
            derived_payload,
            digest_frames,
            distribution_config,
            elf_nul_key_view,
            go_cli_strings,
            key_marker_view,
            library_vectors,
            native_json_key_view,
            npm_help_template,
            nyxid_format_labels,
            pooled_literals,
            portable_ownership,
            reviewed_literals,
        )

        require(len(data) <= MAX_EVIDENCE_BYTES and valid_digest(manifest_sha256)
                and valid_digest(review_sha256) and valid_digest(scanner_manifest_sha256)
                and digest_bytes(data) == manifest_sha256)
        raw = json_bytes(data)
        expected_fields = {"schema", "scanner_manifest_sha256", "packages", "entries"}
        require(isinstance(raw, dict) and raw.keys() in (expected_fields, expected_fields | {"arch_trust"}))
        require(isinstance(raw["schema"], str))
        portable_component = PORTABLE_SCOPES.get(raw["schema"])
        portable = portable_component is not None
        native_arch = portable_component == "runtime-omarchy"
        require((raw["schema"] == "cma-staging-package-evidence-v1" or portable and "arch_trust" not in raw)
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
        entry_fields = PORTABLE_ENTRY_FIELDS if portable else ENTRY_FIELDS
        for row in raw["entries"]:
            require(isinstance(row, dict))
            shape = row.keys() - {"derived_payload", "reviewed_literals", "key_marker_view", "native_json_key_view",
                                  "elf_nul_key_view", "npm_help_template", "nyxid_format_labels", "certificate_blocks",
                                  "pooled_literals"}
            require(shape in (entry_fields, entry_fields | {"self_test"}, entry_fields | {"digest_frames"},
                              entry_fields | {"nested_certificate"}, entry_fields | {"distribution_config"},
                              entry_fields | {"debian_control"}, entry_fields | {"distribution_config", "self_test"}))
            if portable:
                unsupported = ({"nested_certificate", "derived_payload"} if native_arch else {
                    "nested_certificate", "elf_nul_key_view", "distribution_config", "key_marker_view",
                    "native_json_key_view", "certificate_blocks", "pooled_literals",
                })
                require(row["component"] == portable_component and row["member"] is None
                        and type(row["occurrences"]) is int and 1 <= row["occurrences"] <= 2
                        and not unsupported.intersection(row))
            else:
                require(matches(LAYER_ID, row["diff_id"]) and ordinal(row["layer"]) and ordinal(row["entry"]))
            literals = reviewed_literals.validate(row, portable=portable) if "reviewed_literals" in row else {}
            require(not literals or not {"nested_certificate", "debian_control"}.intersection(row))
            derived = "derived_payload" in row
            require(not portable or row["occurrences"] == 1 or derived)
            require(not derived or not {"self_test", "distribution_config", "digest_frames", "debian_control"}.intersection(row))
            marker_views = {"key_marker_view", "elf_nul_key_view"}
            require(not marker_views.intersection(row) or (not derived and shape == entry_fields))
            require("native_json_key_view" not in row or shape == entry_fields)
            literal_helpers = {"npm_help_template", "nyxid_format_labels", "certificate_blocks", "pooled_literals"}
            require(not literal_helpers.intersection(row) or (not derived and literals and shape == entry_fields))
            require(row["component"] in SCOPES and matches(PLATFORM, row["platform"])
                    and valid_digest(row["file_sha256"])
                    and valid_digest(row["package_sha256"]) and row["package_sha256"] in packages
                    and isinstance(row["matches"], list) and bool(row["matches"]))
            package = packages[row["package_sha256"]]
            if portable and "debian_control" in row:
                row = dict(row, path=debian_control.portable_path(row["path"], package))
            path = normalized(row["path"])
            package_member = row["package_member"]
            fields(package_member, {"locator", "path", "type", "sha256"})
            require(matches(NESTED_LOCATOR, package_member["locator"])
                    and package_member["type"] == "regular" and valid_digest(package_member["sha256"])
                    and (derived or package_member["sha256"] == row["file_sha256"])
                    and isinstance(package_member["path"], list)
                    and len(package_member["path"]) == package_member["locator"].count("/") + 1)
            for part in package_member["path"]:
                normalized(part)
            if native_arch:
                owner = row["ownership"]
                fields(owner, {"mtree_sha256"})
                require(valid_digest(owner["mtree_sha256"]) and package["authority_type"] == "arch-signature"
                        and package["architecture"] in {"x86_64", "any"} and row["platform"] == "linux/amd64")
            elif portable:
                owner = row["ownership"]
                require(isinstance(owner, dict))
                needs_source = package["authority_type"] == "pinned-upstream" or derived
                compiled = derived and row["derived_payload"].get("kind") == "compiled-executable"
                expected_owner = {"build_inputs"} if needs_source else set()
                fields(owner, expected_owner | ({"debian_owner"} if compiled else set()))
                require(package["authority_type"] in {"debian-index", "pinned-upstream"}
                        and (not needs_source or portable_ownership.valid_producer_inputs(owner["build_inputs"])))
                if compiled:
                    native = owner["debian_owner"]
                    fields(native, set(portable_ownership.NATIVE_FIELDS))
                    require(all(matches(IDENTIFIER, value) for value in native.values())
                            and native["name"] == package["name"]
                            and native["version"].startswith(package["version"] + "-"))
            if package["authority_type"] != "pinned-upstream" and "debian_control" not in row and not derived:
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
            key = ((row["platform"], path, row["file_sha256"]) if portable else
                   (row["component"], row["platform"], row["layer"], row["diff_id"], row["entry"], locator))
            require(key not in entries)
            go_strings = derived_payload.validate(row, package, portable=portable) if derived else None
            if "npm_help_template" in row:
                npm_help_template.validate(row, package)
            if "nyxid_format_labels" in row:
                nyxid_format_labels.validate(row, package)
            if "certificate_blocks" in row:
                certificate_blocks.validate(row, package)
            if "pooled_literals" in row:
                pooled_literals.validate(row, package)
            key_views = {}
            for name, validator in (("key_marker_view", key_marker_view.validate),
                                    ("native_json_key_view", native_json_key_view.validate)):
                if name in row:
                    validated = validator(row, package, portable=portable)
                    require(not key_views.keys() & validated.keys())
                    key_views.update(validated)
            if "elf_nul_key_view" in row:
                validated = elf_nul_key_view.validate(row, package, portable=native_arch)
                require(not key_views.keys() & validated.keys())
                key_views.update(validated)
            if "nested_certificate" in row:
                entries[key] = (row, validate_nested_certificate(row, package))
                continue
            if "debian_control" in row:
                entries[key] = (row, debian_control.validate(row, package, portable=portable))
                continue
            self_test = library_vectors.validate(row, package, portable=portable) if "self_test" in row else None
            distribution = (distribution_config.validate(row, package, portable=portable)
                            if "distribution_config" in row else None)
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
                if go_strings is not None:
                    go_cli_strings.acceptable_match(match, go_strings)
                elif match["rule"] in PUBLIC_RULES:
                    require(valid_digest(match["candidate_sha256"]) and match["explanation"] in EXPLANATIONS)
                elif match["explanation"] in reviewed_literals.CLASSES:
                    reviewed_literals.acceptable_match(match, literals)
                elif match["explanation"] == KEY_MARKER_VIEW_ADVISORY:
                    key_marker_view.acceptable_match(match, key_views)
                elif match["rule"] == DISTRIBUTION_RULE:
                    distribution_config.acceptable_match(match, distribution)
                elif public_frames:
                    digest_frames.acceptable_match(match)
                else:
                    library_vectors.acceptable_match(match, self_test)
                identity = (match["rule"], match["candidate_sha256"], match["line"])
                require(identity not in verified)
                verified[identity] = match
            if go_strings is not None:
                require(go_strings.keys() == {(rule, sha) for rule, sha, _ in verified})
            if self_test is not None:
                vector_advisory = library_vectors.classification(self_test)
                vector_matches = [match for (rule, _, _), match in verified.items()
                                  if rule in SELF_TEST_RULES and match["explanation"] == vector_advisory]
                require(bool(vector_matches))
                if "executable" in self_test and not portable:
                    require(len(vector_matches) == len(self_test["executable"]["finding_ids"]))
            if public_frames:
                match = verified.get((DIGEST_FRAME_RULE, None, None))
                require(match is not None and match["explanation"] == DIGEST_FRAME_ADVISORY)
            if distribution is not None:
                require((DISTRIBUTION_RULE, distribution["span"]["sha256"], None) in verified)
            for rule in literals:
                require(verified.get((rule, None, None), {}).get("explanation") == reviewed_literals.classification(rule))
            require(key_views.keys() == {match["candidate_sha256"] for match in verified.values()
                                         if match["explanation"] == KEY_MARKER_VIEW_ADVISORY})
            entries[key] = (dict(row, path=path), verified)
        arch = arch_trust_evidence.load_catalog(raw["arch_trust"], packages) if "arch_trust" in raw else None
        return cls(manifest_sha256, review_sha256, scanner_manifest_sha256, packages,
                   {} if portable else entries, arch, entries if portable else None, portable_component)
