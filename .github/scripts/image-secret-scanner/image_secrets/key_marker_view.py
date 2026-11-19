"""Verify a byte-preserving Gitleaks match attributed to public format records."""

import mmap
import re

from . import reviewed_literals
from .common import digest_bytes
from .gate import KEY_MARKER_VIEW_ADVISORY, KEY_MARKER_VIEW_RULE, valid_digest
from .key_material import binary_key_spans
from .openpgp import secret_key_stream
from .package_evidence import LAYER_ID, fields, matches, normalized, ordinal, require

SCOPE = "runtime-omarchy"
PATHS = {"file": "usr/share/file/misc/magic.mgc",
         "shared-mime-info": "usr/share/mime/packages/freedesktop.org.xml"}
MAX_MATCH = 128 * 1024
MARKER_TYPE = rb"-----{}[ \t]+[A-Z0-9 _-]*PRIVATE[ \t]+KEY(?:[ \t]+BLOCK)?[ \t]*-----"
FRAME = {"begin": re.compile(MARKER_TYPE.replace(b"{}", b"BEGIN"), re.I),
         "end": re.compile(MARKER_TYPE.replace(b"{}", b"END"), re.I)}
POSSIBLE_ARMOR = re.compile(rb"[A-Za-z0-9+/=_-]+")


def validate(row, package):
    proof = row["key_marker_view"]
    fields(proof, {"archive_sha256", "image_config", "image_manifest", "matches"})
    require(row["component"] == SCOPE and row["member"] is None and package["authority_type"] == "arch-signature"
            and row["path"] == PATHS.get(package["name"])
            and valid_digest(proof["archive_sha256"]) and matches(LAYER_ID, proof["image_config"])
            and (proof["image_manifest"] is None or matches(LAYER_ID, proof["image_manifest"])))
    entries = proof["matches"]
    require(isinstance(entries, list) and 0 < len(entries) <= 8)
    candidates = {}
    for item in entries:
        fields(item, {"finding_id", "original", "detector", "parts"})
        require(valid_digest(item["finding_id"]))
        original, detector = item["original"], item["detector"]
        for span in (original, detector):
            fields(span, {"offset", "size", "sha256"})
            reviewed_literals.extent(span)
        require(original == detector and 0 < original["size"] <= MAX_MATCH)
        parts = item["parts"]
        require(isinstance(parts, list) and 3 <= len(parts) <= 128)
        for part in parts:
            fields(part, {"kind", "size", "sha256", "source", "line", "proof_sha256"})
            require(type(part["size"]) is int and 0 < part["size"] <= original["size"]
                    and part["kind"] in ("begin", "records", "end")
                    and valid_digest(part["sha256"]) and valid_digest(part["proof_sha256"])
                    and normalized(part["source"]) == part["source"] and ordinal(part["line"]) and part["line"] > 0)
        require(parts[0]["kind"] == "begin" and parts[-1]["kind"] == "end"
                and any(part["kind"] == "records" for part in parts[1:-1])
                and sum(part["size"] for part in parts) == original["size"])
        require(original["sha256"] not in candidates)
        candidates[original["sha256"]] = item
    return candidates


def acceptable_match(match, candidates):
    require(match["rule"] == KEY_MARKER_VIEW_RULE and match["explanation"] == KEY_MARKER_VIEW_ADVISORY
            and valid_digest(match["candidate_sha256"]) and match["candidate_sha256"] in candidates)


def verified_span(data, metadata, item):
    start, end = reviewed_literals.extent(item["original"])
    candidate = data[start:end]
    if (digest_bytes(candidate) != item["original"]["sha256"] or data.find(candidate) != start
            or data.find(candidate, start + 1) != -1 or candidate in metadata):
        return False
    cursor = 0
    first_end = item["parts"][0]["size"]
    last_start = len(candidate) - item["parts"][-1]["size"]
    for part in item["parts"]:
        end = cursor + part["size"]
        body = candidate[cursor:end]
        if digest_bytes(body) != part["sha256"]:
            return False
        frame = FRAME.get(part["kind"])
        if frame is not None and frame.fullmatch(body) is None:
            return False
        cursor = end
    between = candidate[first_end:last_start]
    compact = b"".join(between.replace(b"\0", b"").split())
    return (POSSIBLE_ARMOR.fullmatch(compact) is None and b"proc-type:" not in between.lower()
            and b"dek-info:" not in between.lower())


def verify(path, metadata, proof):
    if not 0 < path.stat().st_size <= reviewed_literals.MAX_BYTES:
        return {}
    with path.open("rb") as source, mmap.mmap(source.fileno(), 0, access=mmap.ACCESS_READ) as data:
        # This policy covers the two native data formats. An ELF-rendered,
        # decoded, normalized, or metadata candidate needs a different proof.
        if (data[:4] == b"\x7fELF" or next(binary_key_spans(data), None) is not None
                or (data[0] & 0x80 and secret_key_stream(path))
                or not reviewed_literals.encoded_material_absent(data)):
            return {}
        return {item["original"]["sha256"]: item for item in proof["matches"]
                if verified_span(data, metadata, item)}
