"""Route independent publications without splitting an AM/runtime contract transition."""
import os
import subprocess

from staging_cd_common import require
from staging_cd_compatibility import GROUP, ContractPolicy
from staging_cd_component_admission import groups, transition_anchor
from staging_cd_github import GitHub
from staging_cd_receipts import canonical
from staging_cd_resolve import output
from staging_cd_runtime import run_private
from staging_cd_set import document
from staging_cd_targets import TARGETS


def routes(matrix, get_policy):
    tasks = groups(matrix)
    # An outstanding transition can outlive every selected source that
    # introduced it. Keep its receipts together through admission.
    coupled = transition_anchor(matrix) is not None
    if not coupled and GROUP & tasks.keys():
        try:
            policy = get_policy()
            sources = {task.get("receipt", task.get("reservation"))["source"]
                       for member in GROUP & tasks.keys() for task in tasks[member]}
            coupled = bool(policy.transition_sources(tasks)) or len({policy.signature(s) for s in sources}) != 1
        except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError):
            # Keep unknown peers together for the strict admission decision.
            coupled = True
    lanes = {c: "contract" if coupled and c in GROUP else c for c in tasks}
    batches = {lane: {"include": [e for e in matrix["include"] if lanes[e["component"]] == lane]}
               for lane in sorted(set(lanes.values()))}
    return lanes, batches


def lane_components(lane):
    require(lane in TARGETS | {"contract"}, "Unknown promotion lane")
    return GROUP if lane == "contract" else {lane}


def check_lane(lane, matrix, release, failures):
    selected = set(groups(matrix))
    decided = set(release["receipts"] if release else {}) | failures.keys()
    require(bool(selected) and selected <= decided <= lane_components(lane),
            "Component admission differs from its scheduled scope")


def main():
    def operation(_masker):
        env = os.environ
        matrix = document(env["CD_PUBLICATION_MATRIX"])
        lanes, batches = routes(matrix, lambda: ContractPolicy(GitHub(env.get("GH_TOKEN")).history()))
        output(env, "lanes", canonical(lanes))
        output(env, "publications", canonical({e["component"]: e["tasks"] for e in matrix["include"]}))
        output(env, "batches", canonical(batches))
        return True
    return run_private(operation, None, subject="component-routes")


if __name__ == "__main__":
    raise SystemExit(main())
