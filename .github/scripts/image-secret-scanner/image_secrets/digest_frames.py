"""Check exact public source coverage for binary detector findings."""

import mmap

from .common import digest_bytes
from .gate import DIGEST_FRAME_ADVISORY, DIGEST_FRAME_RULE, valid_digest
from .key_material import binary_key_spans, frame
from .package_evidence import IDENTIFIER, fields, matches, normalized, ordinal, require
from .policy import PRIVATE

MAX_BYTES = 512 * 1024 * 1024
MAX_FRAME_BYTES = 4096


def validate(row):
    proof = row["digest_frames"]
    fields(proof, {"release", "frames"})
    release = proof["release"]
    fields(release, {"name", "version", "archive_sha256", "authority_sha256"})
    require(row["member"] is None
            and all(matches(IDENTIFIER, release[key]) for key in ("name", "version"))
            and valid_digest(release["archive_sha256"]) and valid_digest(release["authority_sha256"]))
    spans = proof["frames"]
    require(isinstance(spans, list) and 0 < len(spans) <= 128)
    previous = (-1, 0)
    for span in spans:
        fields(span, {"offset", "size", "sha256", "parts"})
        offset, size = span["offset"], span["size"]
        require(type(offset) is int and type(size) is int and 0 < size <= MAX_FRAME_BYTES
                and (offset, size) > previous and offset >= 0 and offset + size <= MAX_BYTES
                and valid_digest(span["sha256"]))
        # The detector may find a second ASN.1 frame in the first frame's digest field.
        previous = (offset, size)
        parts = span["parts"]
        require(isinstance(parts, list) and 2 <= len(parts) <= 16)
        for part in parts:
            fields(part, {"size", "sha256", "source_member", "source_line", "proof_sha256"})
            require(type(part["size"]) is int and 0 < part["size"] <= span["size"]
                    and valid_digest(part["sha256"]) and valid_digest(part["proof_sha256"])
                    and normalized(part["source_member"]) == part["source_member"]
                    and ordinal(part["source_line"]) and part["source_line"] > 0)
        require(sum(part["size"] for part in parts) == span["size"])


def acceptable_match(match):
    require(match["rule"] == DIGEST_FRAME_RULE and match["explanation"] == DIGEST_FRAME_ADVISORY
            and match["candidate_sha256"] is None and match["line"] is None)


def digest_field_start(data):
    outer = frame(data, 0, len(data))
    if not outer or outer[0] != 0x30 or outer[2] != len(data):
        return None
    algorithm = frame(data, outer[1], outer[2])
    if not algorithm or algorithm[0] != 0x30:
        return None
    oid = frame(data, algorithm[1], algorithm[2])
    if not oid or oid[0] != 6 or oid[1] == oid[2] or data[oid[2]:algorithm[2]] not in (b"", b"\x05\x00"):
        return None
    value = frame(data, algorithm[2], outer[2])
    if not value or value[0] != 4 or value[1] == value[2] or value[2] != outer[2]:
        return None
    return value[1]


def matches_public_parts(data, span):
    if digest_bytes(data) != span["sha256"] or digest_field_start(data) != span["parts"][0]["size"]:
        return False
    offset = 0
    for part in span["parts"]:
        end = offset + part["size"]
        if digest_bytes(data[offset:end]) != part["sha256"]:
            return False
        offset = end
    return True


def verify(path, metadata, proof):
    if PRIVATE.search(metadata) or next(binary_key_spans(metadata), None) is not None or not path.stat().st_size:
        return {}
    spans = proof["frames"]
    expected = {(span["offset"], span["size"]): span for span in spans}
    with path.open("rb") as stream, mmap.mmap(stream.fileno(), 0, access=mmap.ACCESS_READ) as data:
        for offset, size in binary_key_spans(data):
            span = expected.pop((offset, size), None)
            if span is None or not matches_public_parts(data[offset:offset + size], span):
                return {}
    return {} if expected else {DIGEST_FRAME_ADVISORY: proof}
