"""Compare reviewed upstream self-test vectors with an authenticated library occurrence."""

import base64
import re

from .common import digest_bytes
from .gate import SELF_TEST_ADVISORY, SELF_TEST_RULES, valid_digest
from .key_material import LIMIT, binary_key_spans
from .package_evidence import IDENTIFIER, LAYER_ID, fields, matches, normalized, ordinal, require
from .policy import PRIVATE

MAX_LIBRARY_BYTES = 16 * 1024 * 1024
LIBRARY = re.compile(r"(?:usr/)?lib(?:/[A-Za-z0-9_+-]+)?/lib[A-Za-z0-9_.+-]+\.so(?:\.[A-Za-z0-9_.-]+)?")
PEM = re.compile(rb"-----BEGIN ([A-Z0-9 ]*PRIVATE KEY)-----[\t ]*\r?\n"
                 rb"([A-Za-z0-9+=/\r\n]+)-----END \1-----[\t \r\n]*")
AGGREGATE = {
    "content.private-key-header": "pem",
    "content.binary-private-key": "binary",
}


def database_paths(package):
    name, version, architecture = (package[k] for k in ("name", "version", "architecture"))
    return {
        "debian-index": {f"var/lib/dpkg/info/{name}.md5sums",
                         f"var/lib/dpkg/info/{name}:{architecture}.md5sums"},
        "arch-signature": {f"var/lib/pacman/local/{name}-{version}/mtree"},
        "apk-index": {"lib/apk/db/installed"},
    }.get(package["authority_type"], set())


def validate(row, package):
    proof = row["self_test"]
    fields(proof, {"database", "upstream", "vectors"})
    require(row["member"] is None and matches(LIBRARY, row["path"]))
    db = proof["database"]
    fields(db, {"path", "sha256", "layer", "diff_id", "entry", "proof_sha256"})
    require(normalized(db["path"]) in database_paths(package)
            and valid_digest(db["sha256"]) and valid_digest(db["proof_sha256"])
            and matches(LAYER_ID, db["diff_id"]) and ordinal(db["entry"])
            and ordinal(db["layer"]) and db["layer"] <= row["layer"])
    upstream = proof["upstream"]
    fields(upstream, {"name", "version", "archive_sha256", "authority_sha256"})
    require(all(matches(IDENTIFIER, upstream[key]) for key in ("name", "version"))
            and valid_digest(upstream["archive_sha256"]) and valid_digest(upstream["authority_sha256"]))
    vectors = proof["vectors"]
    require(isinstance(vectors, list) and 0 < len(vectors) <= 128)
    end = 0
    for v in vectors:
        fields(v, {"offset", "size", "sha256", "kind", "source_member", "source_line", "self_test_symbol"})
        require(type(v["offset"]) is int and v["offset"] >= end and type(v["size"]) is int
                and 0 < v["size"] <= LIMIT and valid_digest(v["sha256"]) and v["kind"] in ("pem", "binary")
                and normalized(v["source_member"]) == v["source_member"]
                and ordinal(v["source_line"]) and v["source_line"] > 0
                and matches(IDENTIFIER, v["self_test_symbol"]))
        end = v["offset"] + v["size"]
        require(end <= MAX_LIBRARY_BYTES)
    return proof


def acceptable_match(match, proof):
    rule = match["rule"]
    require(proof is not None and rule in SELF_TEST_RULES and match["explanation"] == SELF_TEST_ADVISORY)
    if rule in AGGREGATE:
        require(match["candidate_sha256"] is None and match["line"] is None
                and any(v["kind"] == AGGREGATE[rule] for v in proof["vectors"]))
    else:
        require(any(v["kind"] == "pem" and v["sha256"] == match["candidate_sha256"] for v in proof["vectors"]))


def elf_library(data):
    if len(data) < 52 or data[:4] != b"\x7fELF" or data[4] not in (1, 2) or data[5] not in (1, 2):
        return False
    order = "little" if data[5] == 1 else "big"
    return int.from_bytes(data[16:18], order) == 3


def complete_pem(data):
    match = PEM.fullmatch(data)
    if not match:
        return False
    try:
        return bool(base64.b64decode(b"".join(match[2].splitlines()), validate=True))
    except ValueError:
        return False


def verify(data, metadata, proof, rules):
    if not elf_library(data) or PRIVATE.search(metadata) or next(binary_key_spans(metadata), None) is not None:
        return {}
    by_kind = {"pem": [], "binary": []}
    for v in proof["vectors"]:
        payload = data[v["offset"]:v["offset"] + v["size"]]
        if len(payload) != v["size"] or digest_bytes(payload) != v["sha256"]:
            return {}
        if v["kind"] == "pem" and not complete_pem(payload):
            return {}
        by_kind[v["kind"]].append(v)
    vectors = {}
    pem = by_kind["pem"]
    if pem and {m.start() for m in PRIVATE.finditer(data)} == {v["offset"] for v in pem}:
        vectors["content.private-key-header"] = pem
        vectors["gitleaks.private-key"] = pem
    if "content.binary-private-key" in rules and by_kind["binary"]:
        expected = {(v["offset"], v["size"]) for v in by_kind["binary"]}
        if set(binary_key_spans(data)) == expected:
            vectors["content.binary-private-key"] = by_kind["binary"]
    return vectors
