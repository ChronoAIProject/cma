"""Bind reviewed non-secret expressions to exact files, inputs and packages."""

import io
import json
import re
from pathlib import Path

from .common import ScanError, archive_path, json_bytes
from .key_vectors import package_identity_matches, package_matches, sha
from .policy import CREDENTIAL_URL

REGISTRY = Path(__file__).resolve().parents[1] / "reviewed-runtime-content.json"
LIMIT = 512 * 1024 * 1024
RULES = frozenset({"gitleaks.generic-api-key", "gitleaks.image-secret-assignment", "content.credential-url"})


def digest(value):
    return isinstance(value, str) and re.fullmatch(r"[a-f0-9]{64}", value) is not None


def identity(entry):
    return sha(json.dumps(entry, sort_keys=True, separators=(",", ":")).encode())


def policy():
    data = REGISTRY.read_bytes()
    if len(data) > 1024 * 1024:
        raise ScanError("invalid-runtime-content-policy")
    value = json_bytes(data)
    try:
        valid = (
            set(value) == {"schema", "entries"}
            and value["schema"] == "reviewed-runtime-content-v2"
            and isinstance(value["entries"], list)
            and 0 < len(value["entries"]) <= 128
        )
        seen = set()
        for entry in value["entries"]:
            if isinstance(entry.get("path"), dict):
                if (
                    entry["path"] != {"dpkg_control": "triggers"}
                    or entry["metadata"]["kind"] != "dpkg-control"
                ):
                    raise ValueError("invalid control path")
                package = entry["package"]
                entry["path"] = (
                    "var/lib/dpkg/info/" + package["name"] + ":" + package["architecture"] + ".triggers"
                )
            valid &= (
                set(entry)
                == {
                    "components",
                    "architecture",
                    "path",
                    "file_sha256",
                    "bytes",
                    "detector_input",
                    "detector_input_sha256",
                    "package",
                    "metadata",
                    "match",
                    "classification",
                }
                and entry["components"] in (["runtime", "runtime-desktop"], ["runtime-desktop"])
                and entry["architecture"] == "amd64"
                and archive_path(entry["path"]) == entry["path"]
                and type(entry["bytes"]) is int
                and 0 < entry["bytes"] <= LIMIT
                and entry["classification"] == "public-material"
                and entry["detector_input"] in ("file-content", "tar-entry-metadata")
                and digest(entry["file_sha256"])
                and digest(entry["detector_input_sha256"])
            )
            package, metadata, match = entry["package"], entry["metadata"], entry["match"]
            valid &= set(package) - {"archive_sha256"} in (
                {"name", "version"},
                {"name", "version", "architecture"},
            ) and all(isinstance(v, str) and v for v in package.values())
            fields = {
                "dpkg": {"kind", "status", "listing"},
                "dpkg-control": {"kind", "status"},
                "npm": {"kind", "path", "sha256"},
                "release-metadata": {"kind", "path", "sha256"},
                "release-file": {"kind"},
            }
            valid &= set(metadata) == fields.get(metadata["kind"], set())
            valid &= (
                "archive_sha256" not in package
                if metadata["kind"] == "dpkg-control"
                else digest(package.get("archive_sha256"))
            )
            valid &= all(archive_path(p) == p for k, p in metadata.items() if k not in ("kind", "sha256"))
            if metadata["kind"].startswith("dpkg"):
                valid &= package.get("architecture") in ("amd64", "all")
            if metadata["kind"] in ("npm", "release-metadata"):
                valid &= digest(metadata["sha256"])
            if metadata["kind"] == "release-file":
                valid &= package["name"] == "NyxID CLI"
            if entry["detector_input"] == "file-content":
                valid &= entry["detector_input_sha256"] == entry["file_sha256"]
            valid &= match["rule"] in RULES
            if match["rule"] == "content.credential-url":
                valid &= (
                    entry["detector_input"] == "file-content"
                    and set(match) == {"rule", "occurrences"}
                    and isinstance(match["occurrences"], list)
                    and 0 < len(match["occurrences"]) <= 16
                )
                previous = -1
                for occurrence in match["occurrences"]:
                    valid &= (
                        set(occurrence) == {"offset", "length", "sha256", "context_sha256"}
                        and type(occurrence["offset"]) is int
                        and occurrence["offset"] > previous
                        and type(occurrence["length"]) is int
                        and occurrence["length"] > 0
                        and occurrence["offset"] + occurrence["length"] <= entry["bytes"]
                        and digest(occurrence["sha256"])
                        and digest(occurrence["context_sha256"])
                    )
                    previous = occurrence["offset"] + occurrence["length"] - 1
            else:
                valid &= (
                    set(match) == {"rule", "line", "candidate_sha256", "line_sha256"}
                    and type(match["line"]) is int
                    and match["line"] > 0
                    and digest(match["candidate_sha256"])
                    and digest(match["line_sha256"])
                )
            key = identity(entry)
            valid &= key not in seen
            seen.add(key)
    except (KeyError, TypeError, ValueError):
        valid = False
    if not valid:
        raise ScanError("invalid-runtime-content-policy")
    return value["entries"]


def metadata_paths(entry):
    return {v for k, v in entry["metadata"].items() if k not in ("kind", "sha256")}


def package_evidence(state, entry):
    package, metadata = entry["package"], entry["metadata"]
    kind = metadata["kind"]
    if kind == "release-file":
        # The independently reviewed whole executable is the exact release.
        return True
    if kind == "dpkg":
        return package_matches(
            state.get(metadata["status"], b""),
            state.get(metadata["listing"], b""),
            {"package": package, "library": {"path": entry["path"]}},
        )
    if kind == "dpkg-control":
        # Control files are not members of the installed payload .list. Bind
        # their canonical package-owned name plus the installed package record.
        expected = "var/lib/dpkg/info/" + package["name"] + ":" + package["architecture"] + ".triggers"
        return entry["path"] == expected and package_identity_matches(
            state.get(metadata["status"], b""), package
        )
    data = state.get(metadata["path"], b"")
    if sha(data) != metadata["sha256"]:
        return False
    if kind == "release-metadata":
        return True
    try:
        parsed = json_bytes(data)
        return all(parsed.get(key) == package[key] for key in ("name", "version"))
    except (ScanError, ValueError, AttributeError):
        return False


def occurrences(data):
    return [
        {
            "offset": m.start(),
            "length": len(m[0]),
            "sha256": sha(m[0]),
            "context_sha256": sha(data[max(0, m.start() - 64) : m.end() + 64]),
        }
        for m in CREDENTIAL_URL.finditer(data)
    ]


def prove(data, metadata, state, entry):
    payload = data if entry["detector_input"] == "file-content" else metadata
    if (
        len(data) != entry["bytes"]
        or sha(data) != entry["file_sha256"]
        or sha(payload) != entry["detector_input_sha256"]
        or not package_evidence(state, entry)
    ):
        return False
    if entry["match"]["rule"] == "content.credential-url":
        return occurrences(payload) == entry["match"]["occurrences"]
    # Bind the reviewed detector line as well as the whole input. Iteration
    # avoids allocating a line list for large executable inputs.
    return any(
        number == entry["match"]["line"] and sha(line) == entry["match"]["line_sha256"]
        for number, line in enumerate(io.BytesIO(payload), 1)
    )


def finding_matches(finding, entry):
    match = entry["match"]
    if (
        finding.get("scope") != "layer"
        or "member" in finding
        or finding.get("path") != "path-sha256:" + sha(entry["path"].encode())
        or finding.get("rule") != match["rule"]
        or any(type(finding.get(k)) is not int or finding[k] < 0 for k in ("image", "layer", "entry"))
    ):
        return False
    if match["rule"] == "content.credential-url":
        return not any(k in finding for k in ("file_sha256", "line", "candidate_sha256"))
    return finding.get("file_sha256") == entry["detector_input_sha256"] and all(
        finding.get(k) == match[k] for k in ("line", "candidate_sha256")
    )


def reviewed_runtime_content(result, component):
    bound = result["policy"].get("reviewed_runtime_content_sha256")
    if bound is None:
        if "reviewed_runtime_content" in result:
            raise ScanError("unbound-runtime-content-proof")
        return {}
    proofs = result.get("reviewed_runtime_content")
    if bound != sha(REGISTRY.read_bytes()) or not isinstance(proofs, list):
        raise ScanError("unbound-runtime-content-proof")
    entries = {identity(e): e for e in policy() if component in e["components"]}
    findings = {f["finding_id"]: f for f in result["findings"]}
    decisions = {}
    for proof in proofs:
        if not isinstance(proof, dict) or set(proof) != {
            "finding_id",
            "entry_sha256",
            "policy_sha256",
            "file_sha256",
            "detector_input_sha256",
        }:
            raise ScanError("invalid-runtime-content-proof")
        entry = entries.get(proof["entry_sha256"])
        finding = findings.get(proof["finding_id"], {})
        if (
            entry is None
            or proof["finding_id"] in decisions
            or proof["policy_sha256"] != bound
            or proof["file_sha256"] != entry["file_sha256"]
            or proof["detector_input_sha256"] != entry["detector_input_sha256"]
            or not finding_matches(finding, entry)
        ):
            raise ScanError("invalid-runtime-content-proof")
        decisions[proof["finding_id"]] = entry["classification"]
    return decisions
