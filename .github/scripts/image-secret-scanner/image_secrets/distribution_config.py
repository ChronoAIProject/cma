"""Verify a reviewed configuration constant emitted by signed distribution packaging."""

from . import library_vectors
from .common import digest_bytes
from .gate import DISTRIBUTION_ADVISORY, DISTRIBUTION_RULE, valid_digest
from .package_evidence import LAYER_ID, fields, matches, normalized, ordinal, require


def validate(row, package):
    proof = row["distribution_config"]
    fields(proof, {"archive_sha256", "image_config", "image_manifest", "finding_id", "database",
                   "source", "span", "provenance_sha256"})
    require(row["member"] is None and matches(library_vectors.EXECUTABLE, row["path"])
            and all(valid_digest(proof[k]) for k in ("archive_sha256", "finding_id", "provenance_sha256"))
            and matches(LAYER_ID, proof["image_config"])
            and (proof["image_manifest"] is None or matches(LAYER_ID, proof["image_manifest"])))
    library_vectors.validate_database(row, package, proof["database"])
    source = proof["source"]
    fields(source, {"path", "definition_line", "build_argument_line", "published_sha256", "build_recipe_sha256"})
    require(normalized(source["path"]) == source["path"]
            and all(ordinal(source[k]) and source[k] > 0 for k in ("definition_line", "build_argument_line"))
            and valid_digest(source["published_sha256"]) and source["published_sha256"] == source["build_recipe_sha256"])
    span = proof["span"]
    fields(span, {"offset", "size", "sha256"})
    require(type(span["offset"]) is int and span["offset"] >= 0 and type(span["size"]) is int
            and span["size"] == 39 and span["offset"] + span["size"] <= library_vectors.MAX_EXECUTABLE_BYTES
            and valid_digest(span["sha256"]))
    return proof


def acceptable_match(match, proof):
    require(proof is not None and match["rule"] == DISTRIBUTION_RULE
            and match["explanation"] == DISTRIBUTION_ADVISORY and match["line"] is None
            and match["candidate_sha256"] == proof["span"]["sha256"]
            and match["proof_sha256"] == proof["provenance_sha256"])


def verify(data, proof):
    if not library_vectors.database_checksum_matches(data, proof["database"]):
        return False
    offset, size, sha256 = (proof["span"][k] for k in ("offset", "size", "sha256"))
    candidate = data[offset:offset + size]
    return (len(candidate) == size and digest_bytes(candidate) == sha256
            and data.find(candidate) == offset and data.find(candidate, offset + 1) < 0)
