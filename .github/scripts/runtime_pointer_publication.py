"""Move a runtime pointer under the environment's serialized release writer."""

import subprocess
import time

from staging_cd_common import Refusal, require
from staging_cd_registry import RUNTIME_POINTER_TAGS
from staging_cd_set import release_set
from staging_cd_targets import RUNTIME_TARGETS

READINESS_LANES = {"runtime": "agent", "runtime-omarchy": "machine"}


def promote(receipts, publications, history, registry, read_ready, *, environment, fence,
            deadline, clock=time.monotonic, sleep=time.sleep):
    """Publish the admitted candidates and verify promotion within one budget."""
    require(bool(receipts) and receipts.keys() <= RUNTIME_TARGETS, "Runtime pointer release set is invalid")
    release_set(receipts)
    if environment == "production":
        require("runtime-omarchy" not in receipts, "Omarchy pointer is restricted to staging")
    for component, selected in sorted(receipts.items()):
        require(clock() < deadline, "Runtime pointer publication budget expired")
        advance(component, selected, publications[component], history, registry,
                environment=environment, fence=fence)
    return await_active(receipts, read_ready, deadline=deadline, clock=clock, sleep=sleep)


def advance(component, selected, publications, history, registry, *, environment, fence):
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

    write_request = registry.prepare_runtime_pointer(repository, environment, digest)
    require(registry.runtime_pointer(repository, environment) == current,
            "Runtime pointer changed before publication")
    fence({component: selected})
    try:
        registry.write_runtime_pointer(write_request, digest)
    except (Refusal, OSError, subprocess.SubprocessError):
        if registry.runtime_pointer(repository, environment, settle=True) == digest:
            return True
        raise
    require(registry.runtime_pointer(repository, environment, settle=True) == digest,
            "Runtime pointer does not resolve to the selected digest")
    return True


def await_active(receipts, read_ready, *, deadline, clock=time.monotonic, sleep=time.sleep):
    """Verify durable runtime selection on a ready Agent Manager within the deployment budget."""
    remaining = deadline - clock()
    while remaining > 0:
        status = read_ready(remaining)
        if isinstance(status, dict) and status.get("status") == "ready":
            lanes = status.get("runtime_release")
            if isinstance(lanes, dict) and all(
                qualified_lane(lanes.get(READINESS_LANES[component]), receipt["digest"])
                for component, receipt in receipts.items()
            ) and clock() < deadline:
                return lanes
        sleep(min(5, max(0, deadline - clock())))
        remaining = deadline - clock()
    raise Refusal("Agent Manager did not promote the qualified runtime within the deployment budget")


def qualified_lane(lane, digest):
    return (isinstance(lane, dict) and lane.get("digest") == digest
            and type(lane.get("revision")) is int and lane["revision"] > 0)
