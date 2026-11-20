"""Verify a byte-preserving Gitleaks match attributed to public format records."""

import mmap
import re

from . import reviewed_literals
from .common import digest_bytes
from .derived_payload import MIME_TARGETS
from .gate import KEY_MARKER_VIEW_ADVISORY, KEY_MARKER_VIEW_RULE, valid_digest
from .key_material import binary_key_spans
from .mime_markers import marker_ranges
from .openpgp import secret_key_stream
from .package_evidence import fields, image_binding, normalized, ordinal, require

SCOPE = "runtime-omarchy"
PATHS = {"file": "usr/share/file/misc/magic.mgc",
         "shared-mime-info": "usr/share/mime/packages/freedesktop.org.xml"}
MAX_MATCH = 128 * 1024
MARKER_TYPE = rb"-----{}[ \t]+[A-Z0-9 _-]*PRIVATE[ \t]+KEY(?:[ \t]+BLOCK)?[ \t]*-----"
FRAME = {"begin": re.compile(MARKER_TYPE.replace(b"{}", b"BEGIN"), re.I),
         # The detector may end on a second BEGIN delimiter.
         "end": re.compile(MARKER_TYPE.replace(b"{}", b"(?:BEGIN|END)"), re.I)}
POSSIBLE_ARMOR = re.compile(rb"[A-Za-z0-9+/=_-]+")


def validate(row, package, *, portable=False):
    proof = row["key_marker_view"]
    image_binding(proof, {"matches"}, portable=portable)
    origin = row.get("derived_payload")
    # A generated MIME index is accepted only with its reproduced-index proof.
    generated = (isinstance(origin, dict) and origin.get("kind") == "mime-index"
                 and package["name"] == "shared-mime-info" and row["path"] in MIME_TARGETS)
    require(row["component"] == SCOPE and row["member"] is None and package["authority_type"] == "arch-signature"
            and (generated or origin is None and row["path"] == PATHS.get(package["name"])))
    return validate_matches(proof["matches"], portable=portable)


def validate_matches(entries, *, normalize=False, max_parts=128, portable=False):
    require(isinstance(entries, list) and 0 < len(entries) <= 8)
    candidates = {}
    for item in entries:
        span_fields = {"original", "detector", "parts"}
        fields(item, span_fields if portable else span_fields | {"finding_id"})
        require(portable or valid_digest(item["finding_id"]))
        original, detector = item["original"], item["detector"]
        for span in (original, detector):
            fields(span, {"offset", "size", "sha256"})
            reviewed_literals.extent(span)
        require((original != detector if normalize else original == detector)
                and 0 < original["size"] <= MAX_MATCH)
        parts = item["parts"]
        require(isinstance(parts, list) and 3 <= len(parts) <= max_parts)
        for part in parts:
            fields(part, {"kind", "size", "sha256", "source", "line", "proof_sha256"})
            require(type(part["size"]) is int and 0 < part["size"] <= original["size"]
                    and part["kind"] in ("begin", "records", "end")
                    and valid_digest(part["sha256"]) and valid_digest(part["proof_sha256"])
                    and normalized(part["source"]) == part["source"] and ordinal(part["line"]) and part["line"] > 0)
        require(parts[0]["kind"] == "begin" and parts[-1]["kind"] == "end"
                and any(part["kind"] == "records" for part in parts[1:-1])
                and sum(part["size"] for part in parts) == original["size"])
        require(detector["sha256"] not in candidates)
        candidates[detector["sha256"]] = item
    return candidates


def acceptable_match(match, candidates):
    require(match["rule"] == KEY_MARKER_VIEW_RULE and match["explanation"] == KEY_MARKER_VIEW_ADVISORY
            and valid_digest(match["candidate_sha256"]) and match["candidate_sha256"] in candidates)


def verified_span(data, metadata, item, *, end_label=None, records=None):
    start, end = reviewed_literals.extent(item["original"])
    if end > len(data):
        return False
    # In a generated index both frames must be complete published discriminator records.
    if records is not None and ((start, start + item["parts"][0]["size"]) not in records
                                or (end - item["parts"][-1]["size"], end) not in records):
        return False
    candidate = data[start:end]
    if (digest_bytes(candidate) != item["original"]["sha256"] or data.find(candidate) != start
            or data.find(candidate, start + 1) != -1 or candidate in metadata):
        return False
    cursor = 0
    first_end = item["parts"][0]["size"]
    last_start = len(candidate) - item["parts"][-1]["size"]
    for part in item["parts"]:
        part_end = cursor + part["size"]
        if part_end > len(candidate):
            return False
        body = candidate[cursor:part_end]
        if digest_bytes(body) != part["sha256"]:
            return False
        frame = FRAME.get(part["kind"])
        if (frame is not None and frame.fullmatch(body) is None
                and not (part["kind"] == "end" and body == end_label)):
            return False
        cursor = part_end
    if cursor != len(candidate):
        return False
    between = candidate[first_end:last_start]
    compact = b"".join(between.replace(b"\0", b"").split())
    return (POSSIBLE_ARMOR.fullmatch(compact) is None and b"proc-type:" not in between.lower()
            and b"dek-info:" not in between.lower())


def public_subject(data, path):
    return (data[:4] != b"\x7fELF" and next(binary_key_spans(data), None) is None
            and not (data[0] & 0x80 and secret_key_stream(path)) and reviewed_literals.encoded_material_absent(data))


def verify(path, metadata, proof, *, mime_index=None):
    if not 0 < path.stat().st_size <= reviewed_literals.MAX_BYTES:
        return {}
    with path.open("rb") as source, mmap.mmap(source.fileno(), 0, access=mmap.ACCESS_READ) as data:
        # This policy covers the native data formats. An ELF-rendered,
        # decoded, normalized, or metadata candidate needs a different proof.
        if not public_subject(data, path):
            return {}
        records = None if mime_index is None else marker_ranges(data, mime_index)
        return {item["original"]["sha256"]: item for item in proof["matches"]
                if verified_span(data, metadata, item, records=records)}
