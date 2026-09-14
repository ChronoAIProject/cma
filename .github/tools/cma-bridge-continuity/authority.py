"""Exact resource authority for the disposable continuity campaign (#850).

No inventory-wide deletion or production mutation is exposed here. A resource
must belong to the run's namespace and journal before it can be changed.
"""
import hashlib
import json
import re
import secrets
import subprocess
from datetime import datetime, timezone
from pathlib import Path

RUN_LABEL = "cma/continuity-run"
SPEC_LABEL = "cma/continuity-spec"
GLOBAL_KINDS = {"Namespace", "ClusterRole", "ClusterRoleBinding"}
KINDS = GLOBAL_KINDS | {
    "ResourceQuota", "ServiceAccount", "Role", "RoleBinding", "Secret",
    "ConfigMap", "Service", "Pod", "Deployment", "NetworkPolicy",
    "PersistentVolumeClaim", "VolumeSnapshot",
}


def require(condition, code):
    if not condition:
        raise ValueError(code)


def fingerprint(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":")).encode()).hexdigest()


def new_plan(context, owner):
    run = secrets.token_hex(6)
    return {
        "schema": "cma-bridge-continuity-fixture-v1", "run": run,
        "context": context, "owner": owner,
        "namespace": "cma-bridge-e2e-" + run,
        "deployment_id": "cma-bridge-e2e-" + run,
        "created_at": datetime.now(timezone.utc).isoformat(),
        "namespace_uid": None, "resources": [], "sandboxes": [],
    }


def validate_plan(plan):
    require(plan.get("schema") == "cma-bridge-continuity-fixture-v1", "invalid_plan")
    require(re.fullmatch(r"[0-9a-f]{12}", plan.get("run", "")), "invalid_run")
    expected = "cma-bridge-e2e-" + plan["run"]
    require(plan.get("namespace") == expected and plan.get("deployment_id") == expected, "invalid_fixture_scope")
    require(isinstance(plan.get("context"), str) and re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.:/@-]{0,255}", plan["context"]), "invalid_context")
    require(isinstance(plan.get("owner"), str) and len(plan["owner"]) > 0, "owner_required")
    require(isinstance(plan.get("resources"), list) and isinstance(plan.get("sandboxes"), list), "journal_required")


def identity(resource):
    return {"apiVersion": resource["apiVersion"], "kind": resource["kind"],
            "name": resource["metadata"]["name"], "namespace": resource["metadata"].get("namespace")}


def guard_resource(plan, resource):
    validate_plan(plan)
    require(resource.get("kind") in KINDS, "unsupported_fixture_kind")
    metadata = resource.get("metadata", {})
    require(metadata.get("labels", {}).get(RUN_LABEL) == plan["run"], "foreign_run_resource")
    require(isinstance(metadata.get("name"), str) and re.fullmatch(r"[a-z0-9][-a-z0-9.]*", metadata["name"]), "invalid_resource_name")
    if resource["kind"] in GLOBAL_KINDS:
        require(metadata["name"] == plan["namespace"] and "namespace" not in metadata, "foreign_global_resource")
    else:
        require(metadata.get("namespace") == plan["namespace"], "foreign_namespace_resource")
    if resource["kind"] in {"RoleBinding", "ClusterRoleBinding"}:
        subjects = resource.get("subjects", [])
        require(subjects and all(subject.get("kind") == "ServiceAccount" and subject.get("namespace") == plan["namespace"] and subject.get("name") in {"cma-api", "provider"} for subject in subjects), "foreign_role_subject")
        reference = resource.get("roleRef", {})
        if resource["kind"] == "ClusterRoleBinding":
            require(reference == {"apiGroup": "rbac.authorization.k8s.io", "kind": "ClusterRole", "name": plan["namespace"]}, "foreign_cluster_role")
        else:
            require(reference.get("apiGroup") == "rbac.authorization.k8s.io" and reference.get("kind") == "Role" and reference.get("name") in {"cma-api", "provider"}, "foreign_namespaced_role")
    if resource["kind"] == "ClusterRole":
        require(resource.get("rules") == [{"apiGroups": ["node.k8s.io"], "resources": ["runtimeclasses"], "resourceNames": ["gvisor"], "verbs": ["get"]}], "excessive_cluster_authority")
    if resource["kind"] in {"Pod", "Deployment"}:
        pod = resource if resource["kind"] == "Pod" else resource["spec"]["template"]
        require(pod.get("metadata", {}).get("labels", {}).get(RUN_LABEL) == plan["run"], "unmarked_pod")
        spec = pod["spec"]
        require(not any(spec.get(key) for key in ["hostNetwork", "hostPID", "hostIPC"]), "host_namespace_forbidden")
        require(all("hostPath" not in volume for volume in spec.get("volumes", [])), "host_mount_forbidden")
        for container in spec["containers"] + spec.get("initContainers", []):
            require(re.search(r"@sha256:[0-9a-f]{64}$", container.get("image", "")), "immutable_image_required")
            require(not container.get("securityContext", {}).get("privileged"), "privileged_container_forbidden")
            require(all(key in container.get("resources", {}).get("limits", {}) for key in ["cpu", "memory"]), "bounded_container_required")
    return resource


def record_created(plan, resource):
    guard_resource(plan, resource)
    require(isinstance(resource["metadata"].get("uid"), str), "resource_uid_required")
    receipt = {**identity(resource), "uid": resource["metadata"]["uid"],
               "spec_hash": resource["metadata"]["labels"].get(SPEC_LABEL)}
    existing = [entry for entry in plan["resources"] if all(entry[key] == receipt[key] for key in identity(resource))]
    require(not existing or existing == [receipt], "resource_replaced_since_creation")
    if not existing:
        plan["resources"].append(receipt)
    if resource["kind"] == "Namespace":
        require(plan["namespace_uid"] in [None, receipt["uid"]], "namespace_replaced")
        plan["namespace_uid"] = receipt["uid"]
    return receipt


def guard_mutation(plan, namespace, resource):
    guard_resource(plan, namespace)
    require(namespace["kind"] == "Namespace" and plan["namespace_uid"] and namespace["metadata"].get("uid") == plan["namespace_uid"], "namespace_replaced")
    guard_resource(plan, resource)
    wanted = {**identity(resource), "uid": resource["metadata"].get("uid"),
              "spec_hash": resource["metadata"]["labels"].get(SPEC_LABEL)}
    require(wanted in plan["resources"], "unjournaled_or_replaced_resource")
    require(isinstance(resource["metadata"].get("resourceVersion"), str), "resource_version_required")
    return {"uid": wanted["uid"], "resourceVersion": resource["metadata"]["resourceVersion"]}


def save_plan(path, plan):
    validate_plan(plan)
    temporary = Path(str(path) + ".tmp")
    temporary.write_text(json.dumps(plan, indent=2) + "\n")
    temporary.chmod(0o600)
    temporary.replace(path)


def kubectl(plan, args, value=None):
    validate_plan(plan)
    result = subprocess.run(["kubectl", "--context", plan["context"], "--request-timeout=30s", *args],
                            input=None if value is None else json.dumps(value), text=True,
                            capture_output=True, timeout=45)
    # Do not emit Secret bodies or admission errors containing submitted values.
    require(result.returncode == 0, "fixture_kubernetes_request_failed")
    return json.loads(result.stdout) if result.stdout.strip() else None


def create_resource(plan_path, plan, resource):
    guard_resource(plan, resource)
    digest = fingerprint(resource)
    resource["metadata"]["labels"][SPEC_LABEL] = digest[:63]
    if resource["kind"] != "Namespace":
        namespace = kubectl(plan, ["get", "namespace", plan["namespace"], "-o", "json"])
        guard_mutation(plan, namespace, namespace)
    # create never patches an existing workload. An ambiguous result is retained
    # as an intent for exact identity reconciliation before any possible retry.
    intent = Path(plan_path).parent / ("intent-" + resource["kind"].lower() + "-" + resource["metadata"]["name"] + ".json")
    intent.write_text(json.dumps({"identity": identity(resource), "spec_hash": digest[:63]}, indent=2))
    intent.chmod(0o600)
    created = kubectl(plan, ["create", "-f", "-", "-o", "json"], resource)
    record_created(plan, created)
    save_plan(plan_path, plan)
    return created
