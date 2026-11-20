"""Exclude exactly pinned public certificate blocks from one member's encoded-key guard."""

import base64
import binascii
import json

from . import reviewed_literals
from .common import digest_bytes
from .package_evidence import fields, require
from .trust_store import PEM, certificate

RULE = "content.private-key-header"
# tpm2-tss compiles its published TPM vendor root certificates
# (src/tss2-fapi/fapi_certificates.h) into libtss2-fapi. Single base64 lines
# of two of them fit the fail-closed OpenPGP packet heuristic. Only these
# exact blocks, each re-proved certificate-only, skip token decoding.
PIN = {
    "name": "tpm2-tss",
    "version": "4.2.0-2",
    "architecture": "x86_64",
    "artifact_sha256": "7aa2a963f99bcd51e4f3b2587fa8f374a7cfa09ec65749cc9a72fa6f3ad34fdc",
    "path": "usr/lib/libtss2-fapi.so.1.0.0",
    "file_sha256": "77257a83925609c74796a9409d95f3a039554318087352025f11c1bb14004f31",
    "blocks": 32,
    "blocks_sha256": "cf0f40338e3286946b78768f56df26bac0ae3c5cf6f58f9fb09b4048aea2563c",
}


def canonical(blocks):
    return json.dumps(blocks, sort_keys=True, separators=(",", ":")).encode()


def validate(row, package):
    blocks = row["certificate_blocks"]
    require(isinstance(blocks, list) and len(blocks) == PIN["blocks"])
    previous = 0
    for block in blocks:
        fields(block, {"offset", "size", "sha256"})
        start, stop = reviewed_literals.extent(block)
        require(start >= previous)
        previous = stop
    require(row["path"] == PIN["path"] and row["file_sha256"] == PIN["file_sha256"]
            and row["platform"] == "linux/amd64" and row["member"] is None
            and package["authority_type"] == "arch-signature"
            and all(package[key] == PIN[key] for key in ("name", "version", "architecture", "artifact_sha256"))
            and digest_bytes(canonical(blocks)) == PIN["blocks_sha256"]
            and any(entry["rule"] == RULE for entry in row["reviewed_literals"]["findings"]))


def excluded_ranges(data, blocks):
    """Return the pinned ranges once each is re-proved certificate-only, else None."""
    if not blocks:
        return None
    ranges = []
    for block in blocks:
        start, stop = reviewed_literals.extent(block)
        body = data[start:stop]
        match = PEM.fullmatch(body) if stop <= len(data) else None
        if match is None or digest_bytes(body) != block["sha256"]:
            return None
        encoded = match[1].replace(b"\r", b"").replace(b"\n", b"")
        try:
            der = base64.b64decode(encoded, validate=True)
        except binascii.Error:
            return None
        if base64.b64encode(der) != encoded or not certificate(der, lambda _der: None):
            return None
        ranges.append((start, stop))
    return tuple(ranges)
