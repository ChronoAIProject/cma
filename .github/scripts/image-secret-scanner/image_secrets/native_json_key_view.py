"""Reconstruct Go JSON's UTF-8 view of reviewed native format records."""

import mmap
import time

from . import key_marker_view, reviewed_literals
from .common import digest_bytes
from .package_evidence import image_binding, require

ENCODING = "utf8-invalid-byte-replacement"
PUBLIC_KEY_LABEL = b"-----BEGIN PUBLIC KEY-----"
# The signed file 5.48 database is larger than the generated MIME indexes.
MAX_BYTES = 16 * 1024 * 1024
MAX_MATCH_BYTES = 32 * 1024
MAX_RECORD_PARTS = 2048
MAX_RENDER_SECONDS = 5
REPLACEMENTS = dict.fromkeys(range(0xDC80, 0xDD00), 0xFFFD)


def validate(row, package, *, portable=False):
    proof = row["native_json_key_view"]
    image_binding(proof, {"encoding", "matches"}, portable=portable)
    require(proof["encoding"] == ENCODING and row["component"] == key_marker_view.SCOPE
            and row["platform"] == "linux/amd64"
            and row["member"] is None and package["authority_type"] == "arch-signature")
    origin = row.get("derived_payload")
    require((package["name"] == "file" and row["path"] == key_marker_view.PATHS["file"] and origin is None)
            or (not portable and package["name"] == "shared-mime-info" and origin is not None
                and origin["kind"] == "mime-index"))
    candidates = key_marker_view.validate_matches(proof["matches"], normalize=True, max_parts=MAX_RECORD_PARTS,
                                                 portable=portable)
    for item in candidates.values():
        original, detector = item["original"], item["detector"]
        require(original["size"] <= MAX_MATCH_BYTES and detector["size"] <= min(MAX_MATCH_BYTES, 3 * original["size"])
                and original["offset"] + original["size"] <= MAX_BYTES
                and detector["offset"] + detector["size"] <= 3 * MAX_BYTES)
    return candidates


def render(data):
    # Go's JSON string encoder replaces each invalid byte separately.
    return data.decode("utf-8", "surrogateescape").translate(REPLACEMENTS).encode("utf-8")


def verified_view(data, rendered, metadata, rendered_metadata, item):
    start, end = reviewed_literals.extent(item["original"])
    left, right = reviewed_literals.extent(item["detector"])
    if max(end - start, right - left) > MAX_MATCH_BYTES or right > len(rendered):
        return False
    if not key_marker_view.verified_span(data, metadata, item, end_label=PUBLIC_KEY_LABEL):
        return False
    candidate = rendered[left:right]
    if (digest_bytes(candidate) != item["detector"]["sha256"] or candidate in rendered_metadata
            or rendered.find(candidate) != left or rendered.find(candidate, left + 1) != -1):
        return False
    # Separate decoding agrees only when both endpoints are UTF-8 boundaries.
    original = data[start:end]
    return (candidate != original and candidate == render(original)
            and rendered[:left] == render(data[:start]) and rendered[right:] == render(data[end:]))


def verify(path, metadata, proof):
    if not 0 < path.stat().st_size <= MAX_BYTES or len(metadata) > MAX_BYTES:
        return {}
    with path.open("rb") as source, mmap.mmap(source.fileno(), 0, access=mmap.ACCESS_READ) as data:
        if not key_marker_view.public_subject(data, path):
            return {}
        deadline = time.monotonic() + MAX_RENDER_SECONDS
        rendered = render(data[:])
        rendered_metadata = render(metadata)
        if max(len(rendered), len(rendered_metadata)) > 3 * MAX_BYTES:
            return {}
        result = {}
        for item in proof["matches"]:
            if time.monotonic() >= deadline:
                return {}
            if verified_view(data, rendered, metadata, rendered_metadata, item):
                result[item["detector"]["sha256"]] = item
        return result if time.monotonic() < deadline else {}
