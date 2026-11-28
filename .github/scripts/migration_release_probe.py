"""Role and writer-policy binding for an isolated release probe."""
import json

from migration_image_preflight import RunnerError


POLICY_ENV = "CHRONO_TASK_EXPERIENCE_POLICY"
ROLES = ("historical", "current", "candidate", "recovery")
STAGE_ROLES = {"old-write": "historical", "new-read-old-write": "current",
               "new-write": "candidate", "old-read-new-write": "recovery"}


def policy_env(policy):
    if policy not in ("disabled", "enabled"):
        raise RunnerError("explicit writer policy required")
    config = {"mode": "all" if policy == "enabled" else "disabled", "accounts": [],
              "capabilities": ["replay_presentation", "suggested_followup_admission"]
              if policy == "enabled" else []}
    return {"name": POLICY_ENV, "value": json.dumps(config, separators=(",", ":"))}


def declared_reader_artifacts(plan):
    readers = plan.get("readers")
    if not isinstance(readers, list) or len(readers) != len(ROLES):
        raise RunnerError("release probe requires exactly four declared reader roles")
    declared = {}
    for reader in readers:
        if not isinstance(reader, dict):
            raise RunnerError("release reader role is invalid")
        role = reader.get("role")
        if role not in ROLES or reader.get("id") != role or role in declared:
            raise RunnerError("release reader role is missing or duplicated")
        artifact = reader.get("artifact")
        if not isinstance(artifact, dict) or set(artifact) != {"image", "source"}:
            raise RunnerError("release reader artifact is incomplete")
        declared[role] = artifact
    return declared


def declared_artifacts(plan, old, new, policy, inspect):
    selected = {role: old if role in ("historical", "recovery") else new for role in ROLES}
    if plan.get("readers") is None and policy is None:
        return selected
    declared = declared_reader_artifacts(plan)
    for role in ("historical", "candidate"):
        if declared[role] != {key: selected[role][key] for key in ("image", "source")}:
            raise RunnerError("release reader does not match the pinned input")
    for role in ("current", "recovery"):
        if declared[role] != {key: selected[role][key] for key in ("image", "source")}:
            if policy is None:
                raise RunnerError("distinct release readers require an explicit writer policy")
            selected[role] = inspect(**declared[role])
    if policy is not None:
        policy_env(policy)
        if plan.get("candidate") != declared["candidate"]:
            raise RunnerError("release plan candidate does not match declared candidate reader")
    return selected


def stage_schedule(aliases, artifacts, policy):
    stages = (
        ("old-write", "historical-boot"),
        ("new-read-old-write", "current-boot"),
        ("new-write", "candidate-boot" if policy is not None else None),
        ("old-read-new-write", "rollback-boot"),
    )
    schedule = []
    for stage, boot in stages:
        role = STAGE_ROLES[stage]
        schedule.append((stage, boot, aliases[role], artifacts[role],
                         None if policy is None else policy if role == "candidate" else "disabled"))
    return schedule


def policy_container(image, policy):
    return {"name": "api", "image": image, "env": [policy_env(policy)]}


def verify_pod_policy(pods, selected, policy):
    for pod in pods.get("items", []):
        if pod.get("metadata", {}).get("uid") != selected["uid"]:
            continue
        containers = pod.get("spec", {}).get("containers", [])
        values = [entry for container in containers if container.get("name") == "api"
                  for entry in container.get("env", []) if entry.get("name") == POLICY_ENV]
        if values == [policy_env(policy)]:
            return
    raise RunnerError("serving reader writer policy does not match selected control")
