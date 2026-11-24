"""Verify the two compiled Russh format comparisons in staging NyxID."""

import re
import struct

from . import elf_constants, elf_imports, reviewed_literals
from .common import digest_bytes
from .gate import valid_digest
from .package_evidence import fields, matches, require

PATH = "usr/local/bin/nyxid"
RULE = "content.private-key-header"
MAX_BYTES = 64 * 1024 * 1024
COMPARE_BYTES = 21


def validate(row, package):
    proof = row["nyxid_format_labels"]
    fields(proof, {"source_commit", "cargo_lock_sha256", "crate_sha256", "comparisons"})
    require(row["path"] == PATH and row["platform"] == "linux/amd64" and row["member"] is None
            and package["name"] == "nyxid-cli" and package["version"] == "0.10.0"
            and package["authority_type"] == "pinned-upstream" and row["package_member"]["path"][-1].split("/")[-1] == "nyxid"
            and matches(re.compile(r"[a-f0-9]{40}"), proof["source_commit"])
            and all(valid_digest(proof[name]) for name in ("cargo_lock_sha256", "crate_sha256")))
    comparisons = proof["comparisons"]
    require(isinstance(comparisons, list) and len(comparisons) == 2)
    for comparison, expected_size in zip(comparisons, (35, 37)):
        fields(comparison, {"label", "code"})
        for key, size in (("label", expected_size), ("code", COMPARE_BYTES)):
            item = comparison[key]
            fields(item, {"offset", "size", "sha256"})
            _, stop = reviewed_literals.extent(item)
            require(item["size"] == size and stop <= MAX_BYTES)
    first, second = (comparison["label"] for comparison in comparisons)
    require(first["offset"] + first["size"] == second["offset"])
    finding = next((entry for entry in row["reviewed_literals"]["findings"] if entry["rule"] == RULE), None)
    require(finding is not None and len(finding["spans"]) == 2
            and all({key: span[key] for key in ("offset", "size", "sha256")} == comparison["label"]
                    for span, comparison in zip(finding["spans"], comparisons)))


def comparison_target(data, segments, comparison):
    label, code = comparison["label"], comparison["code"]
    start, end = reviewed_literals.extent(label)
    code_start, code_end = reviewed_literals.extent(code)
    address = elf_constants.address_of(segments, start, end - start, (4,))
    instruction = elf_constants.address_of(segments, code_start, code_end - code_start, (5,))
    if address is None or instruction is None or digest_bytes(data[start:end]) != label["sha256"]:
        return None
    body = data[code_start:code_end]
    if (digest_bytes(body) != code["sha256"] or body[0] != 0xBA or body[5:8] != b"\x4c\x89\xff"
            or body[15:17] != b"\xff\x15" or int.from_bytes(body[1:5], "little") != label["size"]):
        return None
    # RDI receives caller data; RSI and EDX hold the signed label and length.
    if elf_constants.rip_load(body[8:15], instruction + 8) != (6, address):
        return None
    disp, = struct.unpack_from("<i", body, 17)
    return instruction + COMPARE_BYTES + disp


def marker_ranges(data, proof):
    if not 0 < len(data) <= MAX_BYTES:
        return set()
    segments = elf_constants.load_segments(data)
    comparisons = proof["comparisons"]
    targets = {comparison_target(data, segments, comparison) for comparison in comparisons}
    if len(targets) != 1 or None in targets:
        return set()
    [slot] = targets
    if not elf_imports.has_import(data, segments, slot, b"bcmp"):
        return set()
    return {reviewed_literals.extent(comparison["label"]) for comparison in comparisons}
