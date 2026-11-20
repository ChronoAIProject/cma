"""Prove exact Go string layout in the reproduced staging GitHub CLI."""

import mmap

from . import elf_constants, reviewed_literals
from .common import digest_bytes
from .gate import GO_CLI_ADVISORY, GO_CLI_RULES, valid_digest
from .package_evidence import fields, normalized, ordinal, require

PATH = "usr/local/bin/gh"
MAX_ELF_BYTES = 128 * 1024 * 1024
EXTENT_FIELDS = {"offset", "size", "sha256"}
REFERENCES = {"word-pair": elf_constants.word_pair, "instruction-pair": elf_constants.instruction_pair,
              "stack-pair": elf_constants.stack_pair}


def extent(value, maximum):
    fields(value, EXTENT_FIELDS)
    start, end = reviewed_literals.extent(value)
    require(end <= MAX_ELF_BYTES and end - start <= maximum)
    return start, end


def validate_reference(reference):
    require(isinstance(reference, dict))
    if reference.get("kind") == "word-pair":
        fields(reference, {"kind", "offset", "sha256"})
        require(type(reference["offset"]) is int and 0 <= reference["offset"] <= MAX_ELF_BYTES - 16
                and valid_digest(reference["sha256"]))
    elif reference.get("kind") == "stack-pair":
        fields(reference, {"kind", "code"})
        extent(reference["code"], 21)
        require(reference["code"]["size"] == 21)
    else:
        fields(reference, {"kind", "address", "length"})
        require(reference["kind"] == "instruction-pair")
        extent(reference["address"], 7)
        extent(reference["length"], 6)
        require(reference["address"]["size"] == 7 and reference["length"]["size"] in (5, 6))


def validate(row, package, proof, *, portable=False):
    require(row["platform"] == "linux/amd64" and row["path"] == PATH and row["member"] is None
            and package["name"] == "github-cli" and package["authority_type"] == "pinned-upstream"
            and row["package_member"]["path"][-1].endswith("/go.sum"))
    cases = proof["strings"]
    require(isinstance(cases, list) and 0 < len(cases) <= len(GO_CLI_RULES))
    checked = {}
    for case in cases:
        fields(case, {"rule", "candidate", "parts"} if portable else {"finding_id", "rule", "candidate", "parts"})
        start, end = extent(case["candidate"], 256)
        identity = case["rule"], case["candidate"]["sha256"]
        require((portable or valid_digest(case["finding_id"])) and case["rule"] in GO_CLI_RULES and identity not in checked
                and isinstance(case["parts"], list) and 2 <= len(case["parts"]) <= 32)
        cursor, before = start, -1
        for part in case["parts"]:
            fields(part, {"literal", "contribution_sha256", "source", "reference"})
            left, right = extent(part["literal"], 512)
            used_start, used_end = max(start, left), min(end, right)
            require(left >= before and used_start == cursor < used_end and valid_digest(part["contribution_sha256"]))
            before, cursor = right, used_end
            source = part["source"]
            fields(source, {"path", "line", "sha256", "proof_sha256"})
            require(normalized(source["path"]) == source["path"] and ordinal(source["line"]) and source["line"] > 0
                    and valid_digest(source["sha256"]) and valid_digest(source["proof_sha256"]))
            validate_reference(part["reference"])
        require(cursor == end)
        checked[identity] = case
    require(len(cases) == len(row["matches"])
            and (portable or {item["finding_id"] for item in cases} == set(proof["findings"])))
    return checked


def acceptable_match(match, evidence):
    require(match["explanation"] == GO_CLI_ADVISORY and (match["rule"], match["candidate_sha256"]) in evidence)


def verify_case(data, segments, metadata, case):
    candidate = case["candidate"]
    start, end = extent(candidate, 256)
    if elf_constants.address_of(segments, start, end - start, (4,)) is None:
        return False
    view = data[start:end]
    # Printable ASCII survives the scanner's ELF text rendering byte for byte.
    if (any(not 32 <= byte < 127 for byte in view) or digest_bytes(view) != candidate["sha256"]
            or view in metadata or data.find(view) != start or data.find(view, start + 1) != -1):
        return False
    for part in case["parts"]:
        literal = part["literal"]
        left, right = extent(literal, 512)
        address = elf_constants.address_of(segments, left, right - left, (4,))
        if (address is None or digest_bytes(data[left:right]) != literal["sha256"]
                or digest_bytes(data[max(left, start):min(right, end)]) != part["contribution_sha256"]):
            return False
        reference = part["reference"]
        if not REFERENCES[reference["kind"]](data, segments, reference, address, right - left):
            return False
    return True


def verify(path, metadata, proof):
    if not 0 < path.stat().st_size <= MAX_ELF_BYTES:
        return {}
    with path.open("rb") as source, mmap.mmap(source.fileno(), 0, access=mmap.ACCESS_READ) as data:
        segments = elf_constants.load_segments(data)
        return {(case["rule"], case["candidate"]["sha256"]): case for case in proof["strings"]
                if verify_case(data, segments, metadata, case)}
