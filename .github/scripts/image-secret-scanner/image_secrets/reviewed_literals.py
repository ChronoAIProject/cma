"""Reproduce reviewed public detector occurrences in a digest-verified member."""

import base64
import binascii
import io
import mmap
import re
import time
from itertools import zip_longest

from . import credential_url, java_class, npm_help_template
from .common import digest_bytes
from .gate import (
    FORMAT_MARKER_ADVISORY,
    FORMAT_MARKER_RULES,
    URL_EXAMPLE_ADVISORY,
    URL_EXAMPLE_RULE,
    valid_digest,
)
from .key_material import binary_key_spans
from .mime_markers import marker_ranges
from .openpgp import PRIVATE_ARMOR, secret_key_packets, secret_key_stream
from .package_evidence import LAYER_ID, fields, matches, normalized, ordinal, require
from .policy import PRIVATE

MAX_BYTES = 512 * 1024 * 1024
MAX_CONTEXT = 16 * 1024
MAX_ENCODED_TOKENS = 100_000
MAX_ENCODED_TOKEN_BYTES = 1024 * 1024
MAX_ENCODED_TOTAL_BYTES = 32 * 1024 * 1024
ENCODED_SCAN_SECONDS = 12
MARKERS = {"content.private-key-header": PRIVATE,
           "content.openpgp-private-key": re.compile(re.escape(PRIVATE_ARMOR))}
RULES = FORMAT_MARKER_RULES | {URL_EXAMPLE_RULE}
CLASSES = frozenset({FORMAT_MARKER_ADVISORY, URL_EXAMPLE_ADVISORY})
ENCODED = re.compile(rb"(?<![A-Za-z0-9+/_=-])[A-Za-z0-9+/_-]{32,}={0,2}(?![A-Za-z0-9+/_=-])")
WRAPPED = re.compile(rb"(?<![A-Za-z0-9+/_=-])[A-Za-z0-9+/_-]{1,76}"
                     rb"(?:(?:[ \t]*\r?\n)+[ \t]*[A-Za-z0-9+/_-]{1,76})+={0,2}(?![A-Za-z0-9+/_=-])")
MIME_TRANSFER_HEADER = re.compile(rb"(?:^|[\x00\r\n])[ \t]*content-transfer-encoding[ \t]*:", re.I)


def classification(rule):
    return URL_EXAMPLE_ADVISORY if rule == URL_EXAMPLE_RULE else FORMAT_MARKER_ADVISORY


def validate(row):
    proof = row["reviewed_literals"]
    fields(proof, {"archive_sha256", "image_config", "image_manifest", "findings"})
    require(valid_digest(proof["archive_sha256"]) and matches(LAYER_ID, proof["image_config"])
            and (proof["image_manifest"] is None or matches(LAYER_ID, proof["image_manifest"])))
    entries = proof["findings"]
    require(isinstance(entries, list) and 0 < len(entries) <= len(RULES))
    by_rule = {}
    for entry in entries:
        fields(entry, {"rule", "finding_id", "spans"})
        rule = entry["rule"]
        require(rule in RULES and rule not in by_rule and valid_digest(entry["finding_id"]))
        spans = entry["spans"]
        require(isinstance(spans, list) and 0 < len(spans) <= 128)
        previous = (-1, 0)
        for span in spans:
            fields(span, {"offset", "size", "sha256", "context", "source"})
            coordinate = extent(span)
            require(coordinate > previous)
            previous = coordinate
            context = span["context"]
            fields(context, {"offset", "size", "sha256"})
            start, stop = extent(context)
            require(start <= coordinate[0] < coordinate[1] < stop and stop - start <= MAX_CONTEXT)
            source = span["source"]
            fields(source, {"path", "line", "sha256"})
            normalized(source["path"])
            require(ordinal(source["line"]) and source["line"] > 0 and valid_digest(source["sha256"]))
        by_rule[rule] = entry
    return by_rule


def extent(value):
    start, size = value["offset"], value["size"]
    require(type(start) is int and start >= 0 and type(size) is int and size > 0
            and start + size <= MAX_BYTES and valid_digest(value["sha256"]))
    return start, start + size


def acceptable_match(match, entries):
    rule = match["rule"]
    require(rule in entries and match["candidate_sha256"] is None and match["line"] is None
            and match["explanation"] == classification(rule))


def occurrences(data, rule):
    if rule == URL_EXAMPLE_RULE:
        return credential_url.authorities(data)
    return ((item.start(), item.end(), False) for item in MARKERS[rule].finditer(data))


def isolated_marker(data, start, end):
    """Require a native string terminator or an explicitly closed source literal."""
    suffix = data[end:end + 1]
    prefix = data[start - 1:start] if start else b""
    return (suffix == b"\0"
            or (prefix == b"\0" and data[end:end + 2] == b"\n\0")
            or (suffix in (b'"', b"'", b"`") and prefix == suffix))


def encoded_material_absent(data):
    limit = time.monotonic() + ENCODED_SCAN_SECONDS
    # An explicit MIME transfer can have multipart and folded boundaries.
    # Token decoding cannot establish complete coverage of that representation.
    if MIME_TRANSFER_HEADER.search(data) is not None:
        return False
    consumed = number = 0
    for pattern in (ENCODED, WRAPPED):
        for candidate in pattern.finditer(data):
            size = candidate.end() - candidate.start()
            number += 1
            consumed += size
            if (number > MAX_ENCODED_TOKENS or size > MAX_ENCODED_TOKEN_BYTES
                    or consumed > MAX_ENCODED_TOTAL_BYTES or time.monotonic() >= limit):
                return False
            encoded = candidate[0] if pattern is ENCODED else b"".join(candidate[0].split())
            try:
                decoded = base64.b64decode(encoded + b"=" * (-len(encoded) % 4), altchars=b"-_", validate=True)
            except binascii.Error:
                continue
            if (PRIVATE.search(decoded) or PRIVATE_ARMOR in decoded
                    or next(binary_key_spans(decoded, deadline=limit), None) is not None
                    or (decoded and decoded[0] & 0x80 and secret_key_packets(io.BytesIO(decoded), len(decoded)))):
                return False
    return time.monotonic() < limit


def exact_spans(data, entry, marker_ranges=None):
    rule = entry["rule"]
    for actual, expected in zip_longest(occurrences(data, rule), entry["spans"]):
        if actual is None or expected is None:
            return False
        start, end, ambiguous = actual
        if ambiguous or (start, end) != extent(expected) or digest_bytes(data[start:end]) != expected["sha256"]:
            return False
        context = expected["context"]
        left, right = extent(context)
        if right > len(data) or digest_bytes(data[left:right]) != context["sha256"]:
            return False
        if rule in FORMAT_MARKER_RULES:
            bounded = (isolated_marker(data, start, end) if marker_ranges is None else (start, end) in marker_ranges)
            if not bounded:
                return False
    return True


def verify(path, metadata, proof, *, mime_index=None, class_member=False, npm_template=None):
    if not 0 < path.stat().st_size <= MAX_BYTES:
        return {}
    with path.open("rb") as stream, mmap.mmap(stream.fileno(), 0, access=mmap.ACCESS_READ) as data:
        boundaries = None if mime_index is None else marker_ranges(data, mime_index)
        if npm_template is not None:
            boundaries = npm_help_template.marker_range(data, npm_template)
        markers = any(entry["rule"] in FORMAT_MARKER_RULES for entry in proof["findings"])
        if markers and class_member:
            boundaries = java_class.utf8_spans(data)
        private = markers and (next(binary_key_spans(data), None) is not None
                               or (data[0] & 0x80 and secret_key_stream(path))
                               or not encoded_material_absent(data))
        return {entry["rule"]: entry for entry in proof["findings"]
                if not (private and entry["rule"] in FORMAT_MARKER_RULES)
                and next(occurrences(metadata, entry["rule"]), None) is None and exact_spans(data, entry, boundaries)}
