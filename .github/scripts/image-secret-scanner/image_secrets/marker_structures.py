"""Additional package and container evidence for exact reviewed marker files."""

import re
import tempfile
from pathlib import Path

from .ar_inventory import inventory
from .arch_vectors import fields
from .common import ScanError, archive_path
from .key_vectors import HEADER, sha
from .mime_markers import mime_matches


def valid_structure(entry):
    structure = entry.get("structure")
    if structure is None:
        return True
    if entry["components"] != ["runtime-omarchy"] or entry["metadata"]["kind"] != "pacman":
        return False
    if structure.get("kind") == "ar":
        member = structure.get("member", {})
        return (
            set(structure) == {"kind", "member"}
            and set(member)
            == {"name", "header_offset", "sha256", "bytes", "marker_offset_in_member", "index"}
            and archive_path(member["name"]) == member["name"]
            and re.fullmatch(r"[a-f0-9]{64}", member["sha256"]) is not None
            and all(
                type(member[k]) is int and member[k] >= 0
                for k in ("header_offset", "bytes", "marker_offset_in_member", "index")
            )
            and 0 < member["bytes"] <= entry["bytes"]
            and member["marker_offset_in_member"] < member["bytes"]
        )
    if structure.get("kind") != "mime":
        return False
    expected = {"kind", "format", "records", "inputs", "generator", "upstream"}
    if structure.get("format") == "cache":
        expected |= {"magic_entries", "format_version"}
    if set(structure) != expected or structure["format"] not in ("magic", "cache"):
        return False
    records, inputs = structure["records"], structure["inputs"]
    if not 0 < len(records) <= 8 or not 0 < len(inputs) <= 32:
        return False
    seen = set()
    for item in [*inputs, structure["generator"]]:
        if (
            not {"path", "sha256"} <= set(item) <= {"path", "sha256", "bytes"}
            or archive_path(item["path"]) != item["path"]
            or item["path"] in seen
            or re.fullmatch(r"[a-f0-9]{64}", item["sha256"]) is None
        ):
            return False
        seen.add(item["path"])
    return True


def extra_paths(entry):
    structure = entry.get("structure", {})
    if structure.get("kind") != "mime":
        return set()
    return {item["path"] for item in structure["inputs"]} | {structure["generator"]["path"]}


def pacman_evidence(state, entry):
    metadata, package = entry["metadata"], entry["package"]
    description = fields(state.get(metadata["status"], b""))
    listing = fields(state.get(metadata["listing"], b""))
    if description is None or listing is None:
        return False
    if any(
        description.get(field) != [value]
        for field, value in (
            ("%NAME%", package["name"]),
            ("%VERSION%", package["version"]),
            ("%ARCH%", "x86_64"),
        )
    ):
        return False
    # Independently reviewed package members bind the artifact digest in policy.
    # Generated caches instead require the owned generator and every pinned input.
    structure = entry.get("structure", {})
    owned = structure["generator"]["path"] if structure.get("kind") == "mime" else entry["path"]
    return listing.get("%FILES%", []).count(owned) == 1


def ar_matches(data, structure):
    expected = structure["member"]
    with tempfile.TemporaryDirectory(prefix="reviewed-ar-") as directory:
        path = Path(directory) / "archive.a"
        path.write_bytes(data)
        try:
            members = inventory(path, 10000)
        except ScanError:
            return False
    index = expected["index"]
    if index >= len(members):
        return False
    member = members[index]
    if (
        member.metadata
        or member.name != expected["name"]
        or member.offset != expected["header_offset"]
        or member.size != expected["bytes"]
    ):
        return False
    content = data[member.start : member.start + member.size]
    return sha(content) == expected["sha256"] and [match.start() for match in HEADER.finditer(content)] == [
        expected["marker_offset_in_member"]
    ]


def structure_matches(data, state, entry):
    structure = entry.get("structure", {})
    if not structure:
        return True
    if structure["kind"] == "ar":
        return ar_matches(data, structure)
    return mime_matches(data, state, structure)
