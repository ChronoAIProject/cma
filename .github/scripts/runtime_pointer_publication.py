"""Move a runtime pointer under the environment's serialized release writer."""

import subprocess

from staging_cd_common import Refusal, require
from staging_cd_registry import RUNTIME_POINTER_TAGS
from staging_cd_set import release_set
from staging_cd_targets import RUNTIME_TARGETS


def advance(component, selected, publications, history, registry, copy, *, environment, fence):
    """Copy the verified manifest, preserving its digest, then read back the exact tag."""
    require(component in RUNTIME_TARGETS and environment in RUNTIME_POINTER_TAGS,
            "Runtime pointer lane or environment is invalid")
    release_set({component: selected})
    repository, digest = selected["repository"], selected["digest"]
    selected_position = history.position(selected["source"])
    require(any(row.get("receipt") == selected and row.get("name") == selected["tag"]
                and row.get("target") == selected["source"] for row in publications),
            "Runtime pointer candidate has no matching publication")
    manifest = registry.manifest(repository, selected["identity"]["version"])
    require(manifest is not None and manifest[0] == digest,
            "Runtime pointer candidate disagrees with its published digest")
    current = registry.runtime_pointer(repository, environment)
    if current == digest:
        return False

    if current is not None:
        previous = [row for row in publications if row.get("receipt", {}).get("digest") == current]
        require(len(previous) == 1, "Runtime pointer predecessor has no unique publication")
        row = previous[0]
        predecessor = row["receipt"]
        release_set({component: predecessor})
        require(predecessor["tag"] == row["name"] and predecessor["source"] == row["target"],
                "Runtime pointer predecessor disagrees with its publication")
        require(history.position(predecessor["source"]) < selected_position,
                "Runtime pointer cannot move to an older or conflicting source")

    fence({component: selected})
    require(registry.runtime_pointer(repository, environment) == current,
            "Runtime pointer changed before publication")
    try:
        copy(repository, digest, RUNTIME_POINTER_TAGS[environment])
    except (Refusal, OSError, subprocess.SubprocessError):
        if registry.runtime_pointer(repository, environment, settle=True) == digest:
            return True
        raise
    require(registry.runtime_pointer(repository, environment, settle=True) == digest,
            "Runtime pointer does not resolve to the selected digest")
    return True
