"""Bind reviewed transformations to the exact staged output and detector finding."""

import re

from . import library_vectors, nested_certificates, reviewed_literals
from .common import digest_bytes
from .gate import (
    FORMAT_MARKER_ADVISORY,
    FORMAT_MARKER_RULES,
    KEY_MARKER_VIEW_ADVISORY,
    KEY_MARKER_VIEW_RULE,
    valid_digest,
)
from .package_evidence import LAYER_ID, fields, matches, require

COMPILED_RULE = "gitleaks.image-hex-secret-assignment"
MIME_TARGETS = frozenset({"usr/share/mime/magic", "usr/share/mime/mime.cache"})
MIME_SOURCE = "usr/share/mime/packages/freedesktop.org.xml"
HEX_CONSTANT = re.compile(rb"[A-Fa-f0-9]{32,128}")


def validate(row, package):
    proof = row["derived_payload"]
    common = {"kind", "archive_sha256", "image_config", "image_manifest", "input_sha256",
              "output_sha256", "producer", "findings"}
    require(isinstance(proof, dict))
    kind = proof.get("kind")
    if kind == "compiled-executable":
        fields(proof, common | {"span"})
        require(row["path"] == "usr/bin/openssl" and row["member"] is None
                and package["name"] == "openssl" and package["authority_type"] == "pinned-upstream")
        span = proof["span"]
        fields(span, {"offset", "size", "sha256"})
        reviewed_literals.extent(span)
        require(32 <= span["size"] <= 128 and len(row["matches"]) == 1)
        [match] = row["matches"]
        require(isinstance(match, dict) and match.get("rule") == COMPILED_RULE
                and match.get("candidate_sha256") == span["sha256"] and match.get("line") is None
                and match.get("explanation") == "public-noncredential-constant")
    elif kind == "repacked-wheel":
        fields(proof, common | {"record_sha256"})
        require(row["component"] == nested_certificates.COMPONENT and row["path"] == nested_certificates.WHEEL
                and row["member"] is not None and package["name"] == "python" and package["authority_type"] == "arch-signature"
                and row["package_member"]["path"][-1] == row["path"] and valid_digest(proof["record_sha256"]))
    else:
        fields(proof, common)
        require(kind == "mime-index" and row["path"] in MIME_TARGETS and row["member"] is None
                and package["name"] == "shared-mime-info" and package["authority_type"] in ("debian-index", "arch-signature")
                and row["package_member"]["path"][-1] == MIME_SOURCE and "reviewed_literals" in row)
        allowed = {(rule, FORMAT_MARKER_ADVISORY) for rule in FORMAT_MARKER_RULES}
        if "native_json_key_view" in row:
            allowed.add((KEY_MARKER_VIEW_RULE, KEY_MARKER_VIEW_ADVISORY))
        require(all(isinstance(match, dict) and (match.get("rule"), match.get("explanation")) in allowed
                    for match in row["matches"]))
    require(all(valid_digest(proof[key]) for key in ("archive_sha256", "input_sha256", "output_sha256"))
            and matches(LAYER_ID, proof["image_config"])
            and (proof["image_manifest"] is None or matches(LAYER_ID, proof["image_manifest"]))
            and proof["input_sha256"] == row["package_member"]["sha256"]
            and proof["output_sha256"] == row["file_sha256"] and proof["input_sha256"] != proof["output_sha256"])
    producer = proof["producer"]
    names = {"attestation_sha256", "identity_sha256", "recipe_sha256", "toolchain_sha256", "inputs_sha256"}
    fields(producer, names)
    require(all(valid_digest(value) for value in producer.values()))
    finding_ids = proof["findings"]
    require(isinstance(finding_ids, list) and 0 < len(finding_ids) == len(row["matches"]) <= 128
            and all(valid_digest(value) for value in finding_ids) and len(set(finding_ids)) == len(finding_ids))


def verify_compiled(data, proof):
    if not library_vectors.elf_subject(data, executable=True):
        return False
    start, end = reviewed_literals.extent(proof["span"])
    candidate = data[start:end]
    return (HEX_CONSTANT.fullmatch(candidate) is not None and digest_bytes(candidate) == proof["span"]["sha256"]
            and data.find(candidate) == start and data.find(candidate, start + 1) == -1)
