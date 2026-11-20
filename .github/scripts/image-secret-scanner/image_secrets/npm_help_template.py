"""Verify the complete placeholder example in the vendored npm help template."""

import re

from . import reviewed_literals
from .common import digest_bytes
from .gate import valid_digest
from .package_evidence import SCOPES, fields, require

MEMBER = "node_modules/@npmcli/config/lib/definitions/definitions.js"
PATH = "usr/local/lib/node_modules/npm/" + MEMBER
RULE = "content.private-key-header"
MAX_TEMPLATE_BYTES = 16 * 1024
OPEN_TEMPLATE = b"description: `"
OPEN_LABEL = b"-----BEGIN " + b"PRIVATE KEY-----"
CLOSE_LABEL = b"-----END " + b"PRIVATE KEY-----"
KEY_FIELD = b'key="'
TEMPLATE_SYNTAX = re.compile(rb"\\[\\`]|[\\`]|\$\{")


def example(line_break):
    return KEY_FIELD + line_break.join((OPEN_LABEL, b"XXXX", b"XXXX", CLOSE_LABEL)) + b'"'


def validate(row, package):
    proof = row["npm_help_template"]
    fields(proof, {"raw_template", "cooked_template_sha256", "raw_example", "cooked_example_sha256"})
    require(row["component"] in SCOPES and row["path"] == PATH and row["member"] is None
            and package["name"] == "npm" and package["authority_type"] == "pinned-upstream"
            and row["package_member"]["path"][-1] == "package/" + MEMBER
            and any(item["rule"] == RULE for item in row["reviewed_literals"]["findings"]))
    for name in ("raw_template", "raw_example"):
        fields(proof[name], {"offset", "size", "sha256"})
    start, end = reviewed_literals.extent(proof["raw_template"])
    left, right = reviewed_literals.extent(proof["raw_example"])
    require(start <= left < right <= end and end - start <= MAX_TEMPLATE_BYTES and right - left <= 160
            and all(valid_digest(proof[name]) for name in ("cooked_template_sha256", "cooked_example_sha256")))


def cook(raw):
    parts = []
    start = 0
    for token in TEMPLATE_SYNTAX.finditer(raw):
        if not token[0].startswith(b"\\") or len(token[0]) != 2:
            return None
        parts.extend((raw[start:token.start()], token[0][1:]))
        start = token.end()
    return b"".join([*parts, raw[start:]]).replace(b"\r\n", b"\n").replace(b"\r", b"\n")


def marker_range(data, proof):
    start, end = reviewed_literals.extent(proof["raw_template"])
    left, right = reviewed_literals.extent(proof["raw_example"])
    line_endings = (b"\r", b"\n")
    if (start < len(OPEN_TEMPLATE) or data[start - len(OPEN_TEMPLATE):start] != OPEN_TEMPLATE
            or data[end:end + 1] != b"`" or data[left - 1:left] not in line_endings
            or data[right:right + 1] not in line_endings):
        return set()
    raw = data[start:end]
    line = data[left:right]
    cooked = cook(raw)
    content = line.lstrip(b" \t")
    if (digest_bytes(raw) != proof["raw_template"]["sha256"] or cooked is None
            or digest_bytes(cooked) != proof["cooked_template_sha256"]
            or digest_bytes(line) != proof["raw_example"]["sha256"] or content != example(b"\\\\n")):
        return set()
    cooked_line = cook(line)
    if (cooked_line is None or cooked_line.lstrip(b" \t") != example(b"\\n")
            or digest_bytes(cooked_line) != proof["cooked_example_sha256"]):
        return set()
    marker = right - len(content) + len(KEY_FIELD)
    return {(marker, marker + len(OPEN_LABEL))}
