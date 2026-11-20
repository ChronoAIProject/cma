"""Prove reviewed public format declarations in a short ELF detector view."""

import mmap
import re
import time

from . import elf_constants, key_marker_view, reviewed_literals
from .binary_view import TEXT_BYTES
from .common import digest_bytes
from .gate import valid_digest
from .key_material import binary_key_spans
from .package_evidence import fields, image_binding, normalized, ordinal, require

ENCODING = "elf-nul-delimited-format-v1"
PATHS = {"libssh2": "usr/lib/libssh2.so.1.0.1", "libssh": "usr/lib/libssh.so.4.12.0",
         "qt6-base": "usr/lib/libQt6Network.so.6.12.0"}
DECLARATIONS = {"libssh2": 3, "libssh": 3, "qt6-base": 4}
MAX_FILE = 16 * 1024 * 1024
MAX_SPAN = 512
MAX_CHECK_SECONDS = 5
DIAGNOSTIC = "libssh2-pem-diagnostic-v1"
# The signed libssh2 release has a public diagnostic whose words fit the broad
# armor alphabet. Qualify only the reviewed source declaration and ELF bytes.
LIBSSH2_DIAGNOSTIC = {
    "artifact_sha256": "462cca91bff394c2719b7e8d1951f0c0a2db979d71521ac93dba6b6b6974f6b9",
    "file_sha256": "8e31bb8758d4f780fd635aa47ca3c0306ccbfbadd7de6c85478a7d712d5ae4b4",
    "literal_sha256": "3c54488a85748de5371734751fb4254842fbd78f6b85bdfe19f4d199d926c5c1",
    "source": "src/pem.c",
    "source_sha256": "eba1a89ba4beb0aa9aaf4e4da68be5f2f890e7367001e8bb705c697b9485d416",
    "lines": (183, 350, 762),
    "version": "1.11.1-7",
    "offset": 268408,
    "size": 41,
}
LABELS = "qt6-network-pem-labels-v1"
PEM_LABEL = re.compile(rb"-----(?:BEGIN|END)(?: [A-Z0-9]+)+-----")
# The signed Qt Network library keeps adjacent published PEM header and footer
# labels. Qualify only the reviewed source lines, ELF bytes and complete span.
QT6_NETWORK_LABELS = {
    "artifact_sha256": "dea8a8c45a7929af115a2ba96c60dff927425f8c65432cf5d1027c79c8aa98f0",
    "file_sha256": "fab5cdd0d9fabe15f1ec0f4b9cc3de42114c594fb3af9515cb26c14990c39ac8",
    "source": "src/network/ssl/qtlsbackend.cpp",
    "source_sha256": "f85aae43f7b96c971031887623a97e9cdb9d1b058c6f5cdb01c1fa305b172579",
    "lines": (1392, 1405, 1407, 1409),
    "version": "6.12.0-3",
    "offset": 1634581,
    "size": 112,
}


def validate(row, package, *, portable=False):
    proof = row["elf_nul_key_view"]
    diagnostic, labels = "middle_diagnostic" in proof, "middle_labels" in proof
    optional = {"middle_diagnostic"} if diagnostic else {"middle_labels"} if labels else set()
    image_binding(proof, {"encoding", "matches"} | optional, portable=portable)
    require(proof["encoding"] == ENCODING and row["component"] == key_marker_view.SCOPE
            and row["platform"] == "linux/amd64" and row["member"] is None
            and package["authority_type"] == "arch-signature" and PATHS.get(package["name"]) == row["path"]
            and labels == (package["name"] == "qt6-base")
            and isinstance(proof["matches"], list) and len(proof["matches"]) == 1)
    [item] = proof["matches"]
    names = {"original", "detector", "declarations"}
    fields(item, names if portable else names | {"finding_id"})
    require(portable or valid_digest(item["finding_id"]))
    for key in ("original", "detector"):
        fields(item[key], {"offset", "size", "sha256"})
    start, stop = reviewed_literals.extent(item["original"])
    require(reviewed_literals.extent(item["detector"]) == (start, stop)
            and stop - start <= MAX_SPAN and stop <= MAX_FILE
            and item["detector"]["sha256"] != item["original"]["sha256"])
    declarations = item["declarations"]
    require(isinstance(declarations, list) and len(declarations) == DECLARATIONS[package["name"]])
    cursor = start
    for index, part in enumerate(declarations):
        fields(part, {"offset", "size", "sha256", "source", "line", "proof_sha256"})
        left, right = reviewed_literals.extent(part)
        require((left > cursor if index else left == cursor) and right <= stop
                and normalized(part["source"]) == part["source"]
                and ordinal(part["line"]) and part["line"] > 0 and valid_digest(part["proof_sha256"]))
        cursor = right
    require(cursor == stop)
    if diagnostic:
        exact = LIBSSH2_DIAGNOSTIC
        middle = declarations[1]
        require(proof["middle_diagnostic"] == DIAGNOSTIC and package["name"] == "libssh2"
                and package["architecture"] == "x86_64" and package["version"] == exact["version"]
                and package["artifact_sha256"] == exact["artifact_sha256"] and row["file_sha256"] == exact["file_sha256"]
                and start == exact["offset"] and middle["sha256"] == exact["literal_sha256"]
                and middle["size"] == exact["size"] and middle["source"] == exact["source"]
                and middle["line"] in exact["lines"] and middle["proof_sha256"] == exact["source_sha256"])
    if labels:
        exact = QT6_NETWORK_LABELS
        require(proof["middle_labels"] == LABELS and package["architecture"] == "x86_64"
                and package["version"] == exact["version"] and package["artifact_sha256"] == exact["artifact_sha256"]
                and row["file_sha256"] == exact["file_sha256"]
                and (start, stop - start) == (exact["offset"], exact["size"])
                and tuple(part["line"] for part in declarations) == exact["lines"]
                and all(part["source"] == exact["source"] and part["proof_sha256"] == exact["source_sha256"]
                        for part in declarations))
    return {item["detector"]["sha256"]: item}


def verified_view(data, rendered, metadata, item, *, diagnostic=False, labels=False):
    start, stop = reviewed_literals.extent(item["original"])
    if elf_constants.address_of(elf_constants.load_segments(data), start, stop - start, (4,)) is None:
        return False
    physical, detector = data[start:stop], rendered[start:stop]
    if (digest_bytes(physical) != item["original"]["sha256"] or digest_bytes(detector) != item["detector"]["sha256"]
            or data.find(physical) != start or data.find(physical, start + 1) != -1
            or rendered.find(detector) != start or rendered.find(detector, start + 1) != -1
            or physical in metadata or detector in metadata.translate(TEXT_BYTES)):
        return False
    cursor = start
    declarations = []
    for part in item["declarations"]:
        left, right = reviewed_literals.extent(part)
        body = data[left:right]
        if (any(data[cursor:left]) or not all(32 <= value < 127 for value in body)
                or digest_bytes(body) != part["sha256"]):
            return False
        declarations.append(body)
        cursor = right
    begin, *inner, end = declarations
    if labels:
        # Every inner declaration is itself a complete published PEM label.
        public_middle = bool(inner) and all(PEM_LABEL.fullmatch(part) is not None for part in inner)
    elif len(inner) != 1:
        return False
    else:
        [middle] = inner
        public_diagnostic = (diagnostic and digest_bytes(middle) == LIBSSH2_DIAGNOSTIC["literal_sha256"]
                             and middle.startswith((b"Unable to ", b"Failed to ", b"Unexpected ", b"Invalid ")))
        public_middle = public_diagnostic or key_marker_view.POSSIBLE_ARMOR.fullmatch(b"".join(middle.split())) is None
    middle = b"\0".join(inner).lower()
    return (key_marker_view.FRAME["begin"].fullmatch(begin) is not None
            and key_marker_view.FRAME["end"].fullmatch(end) is not None
            and public_middle and b"proc-type:" not in middle and b"dek-info:" not in middle)


def verify(path, metadata, proof):
    if not 0 < path.stat().st_size <= MAX_FILE or len(metadata) > MAX_FILE:
        return {}
    with path.open("rb") as source, mmap.mmap(source.fileno(), 0, access=mmap.ACCESS_READ) as data:
        if next(binary_key_spans(data), None) is not None or not reviewed_literals.encoded_material_absent(data):
            return {}
        deadline = time.monotonic() + MAX_CHECK_SECONDS
        rendered = data[:].translate(TEXT_BYTES)
        [item] = proof["matches"]
        diagnostic = proof.get("middle_diagnostic") == DIAGNOSTIC
        labels = proof.get("middle_labels") == LABELS
        if diagnostic and digest_bytes(data) != LIBSSH2_DIAGNOSTIC["file_sha256"]:
            return {}
        if labels and digest_bytes(data) != QT6_NETWORK_LABELS["file_sha256"]:
            return {}
        if (verified_view(data, rendered, metadata, item, diagnostic=diagnostic, labels=labels)
                and time.monotonic() < deadline):
            return {item["detector"]["sha256"]: item}
    return {}
