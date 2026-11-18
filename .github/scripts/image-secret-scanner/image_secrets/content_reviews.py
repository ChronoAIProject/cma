"""Exact non-key content decisions are part of the reviewed scanner bundle."""

import re
from pathlib import Path

from .common import ScanError, digest_file, json_bytes

REGISTRY = Path(__file__).resolve().parents[1] / "content-classifications.json"
COMPONENTS = frozenset({"am", "eg", "web", "image-manager", "runtime", "runtime-desktop", "runtime-omarchy"})
MATCH_FIELDS = frozenset({"rule", "scope", "path", "file_sha256", "candidate_sha256", "line"})
RULES = frozenset({"gitleaks.generic-api-key", "gitleaks.image-secret-assignment"})


def read_registry():
    data = REGISTRY.read_bytes()
    if len(data) > 1024 * 1024:
        raise ScanError("invalid-content-review-registry")
    registry = json_bytes(data)
    if (
        not isinstance(registry, dict)
        or set(registry) != {"schema", "components"}
        or registry.get("schema") != "image-content-classifications-v1"
        or not isinstance(registry.get("components"), dict)
        or set(registry["components"]) != COMPONENTS
    ):
        raise ScanError("invalid-content-review-registry")
    for component, entries in registry["components"].items():
        if not isinstance(entries, list) or len(entries) > 1000:
            raise ScanError("invalid-content-review-entry")
        if component.startswith("runtime") and entries:
            raise ScanError("runtime-content-review-forbidden")
        seen = set()
        for entry in entries:
            if (
                not isinstance(entry, dict)
                or set(entry) != MATCH_FIELDS | {"classification"}
                or entry["rule"] not in RULES
                or entry["classification"] not in ("public-material", "library-example")
                or entry["scope"] not in ("layer", "config")
                or type(entry["line"]) is not int
                or entry["line"] < 1
                or not isinstance(entry["path"], str)
                or re.fullmatch(r"path-sha256:[a-f0-9]{64}", entry["path"]) is None
                or any(
                    not isinstance(entry[key], str) or re.fullmatch(r"[a-f0-9]{64}", entry[key]) is None
                    for key in ("file_sha256", "candidate_sha256")
                )
            ):
                raise ScanError("invalid-content-review-entry")
            identity = tuple(entry[key] for key in sorted(MATCH_FIELDS))
            if identity in seen:
                raise ScanError("duplicate-content-review")
            seen.add(identity)
    return registry


def reviewed_content(result, component):
    if component is None:
        return {}
    if component not in COMPONENTS:
        raise ScanError("invalid-content-review-component")
    registry = read_registry()
    result["policy"]["content_classifications_sha256"] = digest_file(REGISTRY)
    result["policy"]["reviewed_component"] = component
    decisions = {}
    entries = registry["components"][component]
    # Whole-file and candidate hashes plus the exact locator/rule prevent a
    # public example from granting an exception to other data in the same image.
    for entry in entries:
        for finding in result["findings"]:
            if all(finding.get(key) == entry[key] for key in MATCH_FIELDS):
                identity = finding["finding_id"]
                if identity in decisions:
                    raise ScanError("duplicate-content-review")
                decisions[identity] = entry["classification"]
    return decisions
