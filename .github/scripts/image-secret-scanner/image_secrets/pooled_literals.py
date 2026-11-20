"""Bound one reviewed marker inside an exactly pinned x86-64 vector constant pool."""

from . import elf_constants, reviewed_literals
from .common import digest_bytes
from .package_evidence import fields, require
from .policy import PRIVATE

RULE = "content.private-key-header"
VECTOR = 16
# qt6-base inlines its published PKCS#8 labels (src/plugins/tls/shared/
# qtlskey_base.cpp) as seven overlapping 16-byte vector constants in the
# OpenSSL TLS plugin. One marker there has no NUL terminator. Only this exact
# aligned pool may bound it; every other marker keeps its native boundary.
POOL = {"offset": 239888, "size": 112, "sha256": "cdf25ab228a885e49f31c7593822e620095b714702188d48c94fe928ab77b997"}
PIN = {
    "name": "qt6-base",
    "version": "6.12.0-3",
    "architecture": "x86_64",
    "artifact_sha256": "dea8a8c45a7929af115a2ba96c60dff927425f8c65432cf5d1027c79c8aa98f0",
    "path": "usr/lib/qt6/plugins/tls/libqopensslbackend.so",
    "file_sha256": "61c88ac6c5138736c2ff29a916df76853c63cc594b40ff2d1a5cc7736aaa03dd",
    "span": {"offset": 239888, "size": 37, "sha256": "dec643eed02d96980d7003df682d77b28a23f90a6d2a0961886c8953c2e8cb4e"},
    "context": POOL,
    "source": {"path": "src/plugins/tls/shared/qtlskey_base.cpp", "line": 52,
               "sha256": "05f704c056c9a14d1244011ebd2b0054278d51bb80caaf1745f220e50da71077"},
}


def validate(row, package):
    proof = row["pooled_literals"]
    require(isinstance(proof, list) and len(proof) == 1)
    [item] = proof
    fields(item, {"span", "context"})
    for key in ("span", "context"):
        fields(item[key], {"offset", "size", "sha256"})
    require(item["span"] == PIN["span"] and item["context"] == PIN["context"]
            and row["path"] == PIN["path"] and row["file_sha256"] == PIN["file_sha256"]
            and row["platform"] == "linux/amd64" and row["member"] is None
            and package["authority_type"] == "arch-signature"
            and all(package[key] == PIN[key] for key in ("name", "version", "architecture", "artifact_sha256")))
    [finding] = [entry for entry in row["reviewed_literals"]["findings"] if entry["rule"] == RULE]
    require([span for span in finding["spans"] if {key: span[key] for key in ("offset", "size", "sha256")} == PIN["span"]
             and span["context"] == PIN["context"] and span["source"] == PIN["source"]] != [])


def pooled(data, item, *, segments=None):
    """True when the span starts an aligned read-only pool with the pinned bytes."""
    start, end = reviewed_literals.extent(item["span"])
    left, right = reviewed_literals.extent(item["context"])
    segments = elf_constants.load_segments(data) if segments is None else segments
    return (left == start and end <= right <= len(data) and left % VECTOR == 0 and (right - left) % VECTOR == 0
            and digest_bytes(data[left:right]) == item["context"]["sha256"]
            and digest_bytes(data[start:end]) == item["span"]["sha256"]
            and PRIVATE.fullmatch(data[start:end]) is not None
            and elf_constants.address_of(segments, left, right - left, (4,)) is not None)


def marker_ranges(data, proof):
    """Native NUL-terminated marker boundaries, plus the pinned pool span once re-proved."""
    ranges = {(match.start(), match.end()) for match in PRIVATE.finditer(data)
              if reviewed_literals.isolated_marker(data, match.start(), match.end())}
    [item] = proof
    if pooled(data, item):
        ranges.add(reviewed_literals.extent(item["span"]))
    return ranges
