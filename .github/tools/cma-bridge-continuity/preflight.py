"""Read-only access checks before creating any disposable fixture resources."""
from authority import GLOBAL_KINDS, guard_resource, kubectl, require

RESOURCE_NAMES = {
    "Namespace": "namespaces", "ResourceQuota": "resourcequotas",
    "ServiceAccount": "serviceaccounts", "Role": "roles", "RoleBinding": "rolebindings",
    "ClusterRole": "clusterroles", "ClusterRoleBinding": "clusterrolebindings",
    "Secret": "secrets", "ConfigMap": "configmaps", "Service": "services",
    "Pod": "pods", "Deployment": "deployments", "NetworkPolicy": "networkpolicies",
    "PersistentVolumeClaim": "persistentvolumeclaims", "VolumeSnapshot": "volumesnapshots",
}


def check_permissions(plan, resources):
    checks = {}
    for resource in resources:
        guard_resource(plan, resource)
        api = resource["apiVersion"].split("/")
        attributes = {"group": api[0] if len(api) == 2 else "", "verb": "create",
                      "resource": RESOURCE_NAMES[resource["kind"]]}
        if resource["kind"] not in GLOBAL_KINDS:
            attributes["namespace"] = plan["namespace"]
        checks[(attributes["group"], attributes["resource"])] = attributes
    denied = []
    for attributes in checks.values():
        result = kubectl(plan, ["create", "-f", "-", "-o", "json"], {
            "apiVersion": "authorization.k8s.io/v1", "kind": "SelfSubjectAccessReview",
            "spec": {"resourceAttributes": attributes},
        })
        if result.get("status", {}).get("allowed") is not True:
            denied.append(attributes)
    return denied


def require_permissions(plan, resources):
    denied = check_permissions(plan, resources)
    names = ",".join(item["resource"] + ("." + item["group"] if item["group"] else "") for item in denied)
    require(not denied, "fixture_create_permissions_missing:" + names)
