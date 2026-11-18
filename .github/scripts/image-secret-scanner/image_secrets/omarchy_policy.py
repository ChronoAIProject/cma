"""Versioned per-finding Omarchy decisions; never a package or rule exception."""

import json
import re
from pathlib import Path

from .common import ScanError, archive_path, json_bytes
from .key_vectors import sha

REGISTRY = Path(__file__).resolve().parents[1] / "reviewed-omarchy-artifacts.json"
RULES = {
    "path.key-or-certificate",
    "content.private-key-header",
    "content.credential-url",
    "gitleaks.private-key",
    "gitleaks.generic-api-key",
    "gitleaks.image-secret-assignment",
}
FIELDS = {
    "audit_id",
    "component",
    "architecture",
    "path",
    "member",
    "outer",
    "payload",
    "match",
    "provenance",
    "certificate",
    "marker",
    "classification",
}


def digest(value):
    return isinstance(value, str) and re.fullmatch(r"[a-f0-9]{64}", value) is not None


def identity(entry):
    return sha(json.dumps(entry, sort_keys=True, separators=(",", ":")).encode())


def policy():
    data = REGISTRY.read_bytes()
    if len(data) > 4 * 1024 * 1024:
        raise ScanError("invalid-omarchy-artifact-policy")
    value = json_bytes(data)
    try:
        valid = (
            set(value) == {"schema", "review_sha256", "entries"}
            and value["schema"] == "reviewed-omarchy-artifacts-v1"
            and digest(value["review_sha256"])
            and 0 < len(value["entries"]) <= 512
        )
        seen = set()
        for entry in value["entries"]:
            valid &= (
                set(entry) == FIELDS
                and digest(entry["audit_id"])
                and entry["component"] == "runtime-omarchy"
                and entry["architecture"] == "amd64"
                and entry["match"]["rule"] in RULES
                and entry["classification"] in ("public-material", "reviewed-non-key-marker")
            )
            valid &= entry["audit_id"] not in seen
            seen.add(entry["audit_id"])
            for name in entry["path"].split("!"):
                valid &= archive_path(name) == name
            outer = entry["outer"]
            valid &= (
                archive_path(outer["path"]) == outer["path"]
                and entry["path"].split("!", 1)[0] == outer["path"]
                and outer["type"] in ("file", "symlink")
            )
            if outer["type"] == "file":
                valid &= (
                    digest(outer["sha256"])
                    and type(outer["bytes"]) is int
                    and 0 <= outer["bytes"] <= 512 * 1024**2
                )
            else:
                valid &= bool(outer["links"]) and digest(outer["target_sha256"])
                valid &= archive_path(outer["target"]) == outer["target"]
                valid &= all(
                    archive_path(x["path"]) == x["path"] and digest(x["sha256"]) for x in outer["links"]
                )
            member = entry["member"]
            valid &= member is None or bool(re.fullmatch(r"[0-9]+(?:/[0-9]+)*", member))
            valid &= (member is None) == ("!" not in entry["path"])
            if entry["payload"] is not None:
                payload = entry["payload"]
                valid &= (
                    set(payload) == {"sha256", "bytes"}
                    and digest(payload["sha256"])
                    and type(payload["bytes"]) is int
                    and 0 <= payload["bytes"] <= 512 * 1024**2
                )
            match = entry["match"]
            valid &= set(match) <= {
                "rule",
                "candidate_sha256",
                "file_sha256",
                "line",
                "line_sha256",
                "contexts",
            }
            for key in ("candidate_sha256", "file_sha256", "line_sha256"):
                valid &= key not in match or digest(match[key])
            if "line" in match:
                valid &= type(match["line"]) is int and match["line"] > 0 and "line_sha256" in match
            if match["rule"] in ("content.private-key-header", "gitleaks.private-key"):
                valid &= entry["classification"] == "reviewed-non-key-marker"
            else:
                valid &= entry["classification"] == "public-material"
            valid &= isinstance(entry["provenance"], list) and bool(entry["provenance"])
            for proof in entry["provenance"]:
                valid &= proof["kind"] in ("pacman", "npm", "omarchy", "release", "metadata")
                valid &= all(isinstance(proof[k], str) and proof[k] for k in ("name", "version"))
                if proof["kind"] == "pacman":
                    valid &= set(proof) <= {
                        "kind",
                        "name",
                        "version",
                        "architecture",
                        "status",
                        "listing",
                        "owned",
                        "status_sha256",
                        "listing_sha256",
                        "archive_sha256",
                    }
                    valid &= proof["architecture"] in ("x86_64", "any")
                    valid &= all(archive_path(proof[k]) == proof[k] for k in ("status", "listing"))
                    valid &= all(digest(proof[k + "_sha256"]) for k in ("status", "listing"))
                    valid &= proof["owned"] is None or archive_path(proof["owned"]) == proof["owned"]
                if "archive_sha256" in proof:
                    valid &= digest(proof["archive_sha256"])
                if proof["kind"] in ("npm", "metadata"):
                    valid &= archive_path(proof["path"]) == proof["path"] and digest(proof["sha256"])
            if entry["certificate"] is not None:
                valid &= match["rule"] == "path.key-or-certificate"
            if entry["marker"] is not None:
                valid &= digest(entry["marker"])
    except (KeyError, TypeError, ValueError):
        valid = False
    if not valid:
        raise ScanError("invalid-omarchy-artifact-policy")
    return value["entries"]


def finding_matches(finding, entry):
    if (
        finding.get("scope") != "layer"
        or finding.get("path") != "path-sha256:" + sha(entry["path"].encode())
        or finding.get("member") != entry["member"]
        or any(type(finding.get(k)) is not int or finding[k] < 0 for k in ("image", "layer", "entry"))
    ):
        return False
    return all(
        finding.get(k) == entry["match"].get(k) for k in ("rule", "line", "candidate_sha256", "file_sha256")
    )


def reviewed_omarchy_artifacts(result, component):
    bound = result["policy"].get("reviewed_omarchy_artifacts_sha256")
    if bound is None:
        if "reviewed_omarchy_artifacts" in result:
            raise ScanError("unbound-omarchy-artifact-proof")
        return {}
    proofs = result.get("reviewed_omarchy_artifacts")
    if bound != sha(REGISTRY.read_bytes()) or not isinstance(proofs, list):
        raise ScanError("unbound-omarchy-artifact-proof")
    entries = {identity(e): e for e in policy() if e["component"] == component}
    findings = {f["finding_id"]: f for f in result["findings"]}
    decisions = {}
    for proof in proofs:
        if not isinstance(proof, dict) or set(proof) != {"finding_id", "entry_sha256", "policy_sha256"}:
            raise ScanError("invalid-omarchy-artifact-proof")
        entry = entries.get(proof["entry_sha256"])
        if (
            entry is None
            or proof["finding_id"] in decisions
            or proof["policy_sha256"] != bound
            or not finding_matches(findings.get(proof["finding_id"], {}), entry)
        ):
            raise ScanError("invalid-omarchy-artifact-proof")
        decisions[proof["finding_id"]] = entry["classification"]
    return decisions
