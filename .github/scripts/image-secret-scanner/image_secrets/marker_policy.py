"""Exact reviewed parser markers; a parse failure never grants an exception."""

import json
import re
from pathlib import Path

from .common import ScanError, archive_path, json_bytes
from .key_vectors import HEADER, package_matches, sha
from .marker_structures import extra_paths, pacman_evidence, structure_matches, valid_structure

REGISTRY = Path(__file__).resolve().parents[1] / "reviewed-header-markers.json"
LIMIT = 32 * 1024 * 1024


def identity(entry):
    return sha(json.dumps(entry, sort_keys=True, separators=(",", ":")).encode())


def policy():
    value = json_bytes(REGISTRY.read_bytes())
    try:
        valid = (
            set(value) == {"schema", "entries"}
            and value["schema"] == "reviewed-header-markers-v1"
            and isinstance(value["entries"], list)
            and 0 < len(value["entries"]) <= 64
        )
        seen = set()
        for entry in value["entries"]:
            valid &= (
                set(entry) - {"structure"}
                == {
                    "components",
                    "architecture",
                    "path",
                    "sha256",
                    "bytes",
                    "classification",
                    "package",
                    "metadata",
                    "markers",
                }
                and entry["components"]
                in (["runtime", "runtime-desktop"], ["runtime-desktop"], ["runtime-omarchy"])
                and entry["architecture"] == "amd64"
                and archive_path(entry["path"]) == entry["path"]
                and type(entry["bytes"]) is int
                and 0 < entry["bytes"] <= LIMIT
                and entry["classification"] in ("bare-parser-marker", "npm-placeholder")
                and 0 < len(entry["markers"]) <= 8
            )
            package, metadata = entry["package"], entry["metadata"]
            valid &= (
                set(package) == {"name", "version", "artifact_kind", "sha256", "source_sha256"}
                and package["artifact_kind"] in ("package", "upstream-source")
                and all(isinstance(package[k], str) and package[k] for k in ("name", "version"))
            )
            expected_fields = {
                "dpkg": {"kind", "status", "listing"},
                "npm": {"kind", "path"},
                "release-file": {"kind"},
                "pacman": {"kind", "status", "listing"},
            }
            valid &= set(metadata) == expected_fields.get(metadata["kind"], set())
            for key, path in metadata.items():
                if key != "kind":
                    valid &= archive_path(path) == path
            hashes = [entry["sha256"], package["sha256"], package["source_sha256"]]
            last = -1
            for marker in entry["markers"]:
                valid &= (
                    set(marker) == {"offset", "length", "sha256", "following_sha256"}
                    and type(marker["offset"]) is int
                    and marker["offset"] > last
                    and type(marker["length"]) is int
                    and 1 <= marker["length"] <= 128
                    and marker["offset"] + marker["length"] <= entry["bytes"]
                )
                last = marker["offset"] + marker["length"] - 1
                hashes.extend((marker["sha256"], marker["following_sha256"]))
            valid &= all(isinstance(d, str) and re.fullmatch(r"[a-f0-9]{64}", d) for d in hashes)
            valid &= valid_structure(entry)
            key = (tuple(entry["components"]), entry["path"], entry["sha256"])
            valid &= key not in seen
            seen.add(key)
    except (KeyError, TypeError, ValueError):
        valid = False
    if not valid:
        raise ScanError("invalid-reviewed-marker-policy")
    return value["entries"]


def metadata_paths(entry):
    return {value for key, value in entry["metadata"].items() if key != "kind"} | extra_paths(entry)


def package_evidence(state, entry):
    metadata, package = entry["metadata"], entry["package"]
    if metadata["kind"] == "release-file":
        # The reviewed whole executable identifies this exact release artifact.
        # There is no installed package manager record and no image code executes.
        return True
    if metadata["kind"] == "dpkg":
        expected = {
            "package": dict(package, architecture=entry["architecture"]),
            "library": {"path": entry["path"]},
        }
        return package_matches(
            state.get(metadata["status"], b""), state.get(metadata["listing"], b""), expected
        )
    if metadata["kind"] == "pacman":
        return pacman_evidence(state, entry)
    try:
        installed = json_bytes(state.get(metadata["path"], b""))
        return installed.get("name") == package["name"] and installed.get("version") == package["version"]
    except (ValueError, AttributeError, ScanError):
        return False


def occurrences(data):
    return [
        {
            "offset": match.start(),
            "length": len(match[0]),
            "sha256": sha(match[0]),
            "following_sha256": sha(data[match.end() : match.end() + 256]),
        }
        for match in HEADER.finditer(data)
    ]


def prove(data, state, entry):
    if (
        len(data) != entry["bytes"]
        or sha(data) != entry["sha256"]
        or occurrences(data) != entry["markers"]
        or not package_evidence(state, entry)
        or not structure_matches(data, state, entry)
    ):
        return None
    return {
        "entry_sha256": identity(entry),
        "file_sha256": entry["sha256"],
        "markers": entry["markers"],
        "policy_sha256": sha(REGISTRY.read_bytes()),
    }


def reviewed_markers(result, component):
    digest = sha(REGISTRY.read_bytes())
    if "reviewed_markers_sha256" not in result["policy"]:
        if "reviewed_markers" in result:
            raise ScanError("unbound-reviewed-marker-proof")
        return set()
    proofs = result.get("reviewed_markers")
    if result["policy"]["reviewed_markers_sha256"] != digest or not isinstance(proofs, list):
        raise ScanError("unbound-reviewed-marker-proof")
    entries = {identity(entry): entry for entry in policy() if component in entry["components"]}
    candidates = {item["finding_id"]: item for item in result["findings"]}
    verified = set()
    for proof in proofs:
        if not isinstance(proof, dict) or set(proof) - {"member"} != {
            "entry_sha256",
            "file_sha256",
            "markers",
            "policy_sha256",
            "finding_id",
        }:
            raise ScanError("invalid-reviewed-marker-proof")
        entry = entries.get(proof["entry_sha256"])
        finding = candidates.get(proof["finding_id"], {})
        path = entry["path"] if entry else ""
        nested = proof.get("member")
        if nested is not None:
            structure = entry.get("structure", {}) if entry else {}
            member = structure.get("member", {})
            if structure.get("kind") != "ar" or nested != str(member.get("index")):
                raise ScanError("invalid-reviewed-marker-proof")
            path += "!" + member["name"]
        if (
            entry is None
            or proof["finding_id"] in verified
            or proof["policy_sha256"] != digest
            or proof["file_sha256"] != entry["sha256"]
            or proof["markers"] != entry["markers"]
            or finding.get("rule") != "content.private-key-header"
            or finding.get("scope") != "layer"
            or ("member" in finding) != ("member" in proof)
            or finding.get("member") != nested
            or finding.get("path") != "path-sha256:" + sha(path.encode())
            or any(
                type(finding.get(key)) is not int or finding[key] < 0 for key in ("image", "layer", "entry")
            )
        ):
            raise ScanError("invalid-reviewed-marker-proof")
        verified.add(proof["finding_id"])
    return verified
