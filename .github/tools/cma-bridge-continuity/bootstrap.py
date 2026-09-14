"""Provision an isolated previous-version CMA environment in the selected cluster.

Usage: bootstrap.py REPOSITORY PRIVATE_RUN_DIRECTORY CONTEXT OWNER_SUBJECT
Production supplies only immutable image/config references and the existing
NyxID application/storage client credentials. Mongo, encryption, provider key,
provider server, service accounts, home seed, and runtime namespace are separate.
"""
import base64
import copy
import json
import secrets
import sys
from pathlib import Path

import yaml

from authority import create_resource, kubectl, new_plan, require, save_plan
from preflight import require_permissions

MONGO_IMAGE = "mongo@sha256:108c3c1646aa75e8f75361024ae3f90b2ba9d2ad6d442d042bdeb269ef8d15bf"


def build_resources(repo, plan, production):
    ns = plan["namespace"]
    labels = {"cma/continuity-run": plan["run"]}

    def resource(kind, name, api="v1", **fields):
        metadata = {"name": name, "labels": dict(labels)}
        if kind not in {"Namespace", "ClusterRole", "ClusterRoleBinding"}:
            metadata["namespace"] = ns
        return {"apiVersion": api, "kind": kind, "metadata": metadata, **fields}

    def pod_spec(name, container, volumes=(), service_account=None):
        container.setdefault("securityContext", {"allowPrivilegeEscalation": False, "capabilities": {"drop": ["ALL"]}})
        container.setdefault("resources", {"requests": {"cpu": "100m", "memory": "128Mi"}, "limits": {"cpu": "1", "memory": "1Gi"}})
        spec = {"containers": [container], "volumes": list(volumes), "automountServiceAccountToken": bool(service_account),
                "securityContext": {"seccompProfile": {"type": "RuntimeDefault"}}}
        if service_account:
            spec["serviceAccountName"] = service_account
        return {"metadata": {"labels": {**labels, "app": name}}, "spec": spec}

    def deployment(name, template):
        return resource("Deployment", name, "apps/v1", spec={"replicas": 1, "strategy": {"type": "Recreate"},
                        "selector": {"matchLabels": {"app": name}}, "template": template})

    def service(name, port):
        return resource("Service", name, spec={"selector": {"app": name}, "ports": [{"port": port, "targetPort": port}]})

    resources = [resource("Namespace", ns), resource("ResourceQuota", "continuity-budget", spec={"hard": {
        "requests.cpu": "4", "limits.cpu": "8", "requests.memory": "8Gi", "limits.memory": "16Gi",
        "pods": "12", "persistentvolumeclaims": "6", "requests.storage": "512Gi",
    }})]
    for name in ["cma-api", "provider", "sandbox-runner"]:
        resources.append(resource("ServiceAccount", name, automountServiceAccountToken=False))
    role = next(doc for doc in yaml.safe_load_all((repo / "infra/api/serviceaccount.yaml").read_text()) if doc["kind"] == "Role")
    resources += [resource("Role", "cma-api", "rbac.authorization.k8s.io/v1", rules=role["rules"]),
                  resource("RoleBinding", "cma-api", "rbac.authorization.k8s.io/v1", roleRef={"apiGroup": "rbac.authorization.k8s.io", "kind": "Role", "name": "cma-api"},
                           subjects=[{"kind": "ServiceAccount", "name": "cma-api", "namespace": ns}])]
    provider_rules = [rule for rule in production["provider_role"]["rules"] if rule["apiGroups"] != ["node.k8s.io"]]
    resources += [resource("Role", "provider", "rbac.authorization.k8s.io/v1", rules=provider_rules),
                  resource("RoleBinding", "provider", "rbac.authorization.k8s.io/v1", roleRef={"apiGroup": "rbac.authorization.k8s.io", "kind": "Role", "name": "provider"},
                           subjects=[{"kind": "ServiceAccount", "name": "provider", "namespace": ns}]),
                  resource("ClusterRole", ns, "rbac.authorization.k8s.io/v1", rules=[{"apiGroups": ["node.k8s.io"], "resources": ["runtimeclasses"], "resourceNames": ["gvisor"], "verbs": ["get"]}]),
                  resource("ClusterRoleBinding", ns, "rbac.authorization.k8s.io/v1", roleRef={"apiGroup": "rbac.authorization.k8s.io", "kind": "ClusterRole", "name": ns},
                           subjects=[{"kind": "ServiceAccount", "name": "provider", "namespace": ns}])]

    # Runtime Pods cannot reach other namespaces, node metadata, or this run's
    # MongoDB. The public model/service endpoints and DNS remain available.
    resources.append(resource("NetworkPolicy", "runtime-isolation", "networking.k8s.io/v1", spec={
        "podSelector": {"matchLabels": {"opensandbox.io/workload": "sandbox"}}, "policyTypes": ["Ingress", "Egress"],
        "ingress": [{"from": [{"podSelector": {"matchLabels": {"app": "provider"}}}, {"podSelector": {"matchLabels": {"app": "cma-api"}}}]}],
        "egress": [{"to": [{"namespaceSelector": {"matchLabels": {"kubernetes.io/metadata.name": "kube-system"}}}], "ports": [{"port": 53, "protocol": "UDP"}, {"port": 53, "protocol": "TCP"}]},
                   {"to": [{"ipBlock": {"cidr": "0.0.0.0/0", "except": ["169.254.169.254/32", "10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16"]}}]}],
    }))
    resources.append(resource("NetworkPolicy", "mongo-isolation", "networking.k8s.io/v1", spec={
        "podSelector": {"matchLabels": {"app": "mongo"}}, "policyTypes": ["Ingress"],
        "ingress": [{"from": [{"podSelector": {"matchLabels": {"app": "cma-api"}}}, {"podSelector": {"matchLabels": {"app": "mongo"}}}], "ports": [{"port": 27017}]}],
    }))
    mongo = pod_spec("mongo", {"name": "mongo", "image": MONGO_IMAGE,
        "command": ["mongod", "--replSet", "continuity", "--bind_ip_all", "--wiredTigerCacheSizeGB", "0.25"],
        "ports": [{"containerPort": 27017}], "volumeMounts": [{"name": "data", "mountPath": "/data/db"}],
        "readinessProbe": {"tcpSocket": {"port": 27017}, "initialDelaySeconds": 3, "periodSeconds": 3},
    }, [{"name": "data", "emptyDir": {"sizeLimit": "2Gi"}}])
    mongo["spec"]["securityContext"].update(runAsUser=999, runAsGroup=999, fsGroup=999)
    mongo["spec"]["restartPolicy"] = "Never"
    resources += [resource("Pod", "mongo", spec=mongo["spec"]), service("mongo", 27017)]
    resources[-2]["metadata"]["labels"].update(app="mongo")

    provider_key = secrets.token_hex(32)
    resources.append(resource("Secret", "provider-key", type="Opaque", stringData={"opensandbox-api-key": provider_key}))
    provider_config = f'''[server]
host = "0.0.0.0"
port = 8080
max_sandbox_timeout_seconds = 3600
[log]
level = "INFO"
[runtime]
type = "kubernetes"
execd_image = "opensandbox/execd:v1.0.20"
[tenants]
provider = "file"
[kubernetes]
namespace = "{ns}"
service_account = "sandbox-runner"
workload_provider = "batchsandbox"
batchsandbox_template_file = "/etc/opensandbox/template.yaml"
sandbox_create_timeout_seconds = 300
informer_enabled = false
[secure_runtime]
type = "gvisor"
k8s_runtime_class = "gvisor"
[ingress]
mode = "direct"
'''
    template = yaml.safe_load(production["provider_template"]["data"]["batchsandbox-template.yaml"])
    template["metadata"]["labels"].update(labels)
    template["spec"]["template"]["metadata"]["labels"].update(labels)
    resources.append(resource("ConfigMap", "provider", data={"config.toml": provider_config, "template.yaml": yaml.safe_dump(template)}))
    startup = '''python3 -c 'import json,pathlib,os; key=pathlib.Path("/var/secrets/opensandbox-api-key").read_text().strip(); pathlib.Path("/tmp/tenants.toml").write_text("[[tenants]]\\nname = \\"continuity\\"\\nnamespace = " + json.dumps(os.environ["FIXTURE_NAMESPACE"]) + "\\napi_keys = [" + json.dumps(key) + "]\\n")'
export SANDBOX_TENANTS_CONFIG_PATH=/tmp/tenants.toml
exec opensandbox-server --config /etc/opensandbox/config.toml
'''
    provider = pod_spec("provider", {"name": "provider", "image": production["provider"]["spec"]["template"]["spec"]["containers"][0]["image"],
        "command": ["/bin/sh", "-eu", "-c", startup], "env": [{"name": "FIXTURE_NAMESPACE", "value": ns}],
        "ports": [{"containerPort": 8080}], "volumeMounts": [{"name": "config", "mountPath": "/etc/opensandbox", "readOnly": True}, {"name": "key", "mountPath": "/var/secrets", "readOnly": True}],
        "readinessProbe": {"tcpSocket": {"port": 8080}, "initialDelaySeconds": 3, "periodSeconds": 3},
    }, [{"name": "config", "configMap": {"name": "provider"}}, {"name": "key", "secret": {"secretName": "provider-key"}}], "provider")
    resources += [deployment("provider", provider), service("provider", 8080)]

    # Reproduce the current persistent-home format using a new credential-free
    # seed, never a user's PVC, native conversation, checkpoint or home.
    seed_docs = list(yaml.safe_load_all((repo / "infra/storage/home-seed.yaml").read_text()))
    for original in [doc for doc in seed_docs if doc["kind"] in {"PersistentVolumeClaim", "Pod"}]:
        seed = resource(original["kind"], "cma-home-seed-v1", spec=copy.deepcopy(original["spec"]))
        if seed["kind"] == "Pod":
            seed["spec"]["containers"][0]["image"] = production["config"]["data"]["CHRONO_SANDBOX_IMAGE"]
        resources.append(seed)

    settings = copy.deepcopy(production["config"]["data"])
    settings.update(CHRONO_MONGODB_DATABASE=ns, CHRONO_DEPLOYMENT_ID=ns, CHRONO_SANDBOX_NAMESPACE=ns,
                    CHRONO_SANDBOX_BASE_URL=f"http://provider.{ns}.svc.cluster.local:8080",
                    CHRONO_SERVER_PUBLIC_URL="https://cma-continuity.test", CHRONO_SERVER_CORS_ORIGIN="https://cma-continuity.test",
                    CHRONO_SERVER_TRUSTED_PROXY_CIDRS="", CHRONO_SERVER_CLOUDFLARE_PROXY_CIDRS="",
                    CHRONO_SANDBOX_API_KEY_FILE="/var/secrets/opensandbox-api-key")
    # Replaced with this run's ready snapshot UID before the API is created.
    settings["CHRONO_SANDBOX_HOME_TEMPLATE_UID"] = "awaiting-fixture-seed"
    api_secrets = {key: production["secrets"]["data"][key] for key in ["CHRONO_NYXID_CLIENT_SECRET", "CHRONO_STORAGE_CLIENT_SECRET"]}
    api_secrets.update(CHRONO_AUTH_ENCRYPTION_KEY=base64.b64encode(base64.b64encode(secrets.token_bytes(32))).decode(),
                       CHRONO_MONGODB_URI=base64.b64encode(f"mongodb://mongo.{ns}.svc.cluster.local:27017/?replicaSet=continuity".encode()).decode())
    resources.append(resource("Secret", "cma-api", type="Opaque", data=api_secrets))

    api = copy.deepcopy(production["api"]["spec"]["template"])
    api["metadata"] = {"labels": {**labels, "app": "cma-api"}}
    api["spec"]["serviceAccountName"] = "cma-api"
    api["spec"].pop("affinity", None)
    api["spec"]["volumes"] = [{"name": "tmp", "emptyDir": {}}, {"name": "opensandbox-api-key", "secret": {"secretName": "provider-key"}}]
    for container in api["spec"]["containers"]:
        container["envFrom"] = [{"configMapRef": {"name": "cma-api"}}, {"secretRef": {"name": "cma-api"}}]
    api_resources = [resource("ConfigMap", "cma-api", data=settings), deployment("cma-api", api), service("cma-api", 8080)]
    snapshot = yaml.safe_load((repo / "infra/storage/home-template.yaml").read_text())
    snapshot["metadata"] = {"name": "cma-home-seed-v1", "namespace": ns, "labels": labels,
                            "annotations": {"cma/home-seed-format": "1", "cma/home-seed-image": settings["CHRONO_SANDBOX_IMAGE"].split("@", 1)[1]}}
    return resources, snapshot, api_resources


def main():
    require(len(sys.argv) == 5, "usage_repository_private_directory_context_owner")
    repo, directory = Path(sys.argv[1]).resolve(), Path(sys.argv[2]).resolve()
    require(not directory.exists(), "fresh_run_directory_required")
    directory.mkdir(mode=0o700)
    plan = new_plan(sys.argv[3], sys.argv[4])
    plan_path = directory / "plan.json"
    save_plan(plan_path, plan)
    refs = {
        "config": ["-n", "chrono-bot", "get", "configmap", "cma-api"],
        "secrets": ["-n", "chrono-bot", "get", "secret", "cma-api"],
        "api": ["-n", "chrono-bot", "get", "deployment", "cma-api"],
        "provider": ["-n", "opensandbox-system", "get", "deployment", "opensandbox-server"],
        "provider_role": ["get", "clusterrole", "opensandbox-server-role"],
        "provider_template": ["-n", "opensandbox-system", "get", "configmap", "opensandbox-batchsandbox-template"],
    }
    production = {name: kubectl(plan, [*args, "-o", "json"]) for name, args in refs.items()}
    resources, snapshot, api_resources = build_resources(repo, plan, production)
    from authority import guard_resource
    for item in resources + [snapshot] + api_resources:
        guard_resource(plan, item)
    for name, items in [("bootstrap-private.json", resources), ("snapshot.json", [snapshot]), ("api-private.json", api_resources)]:
        path = directory / name
        path.write_text(json.dumps(items, indent=2))
        path.chmod(0o600)
    require_permissions(plan, resources + [snapshot] + api_resources)
    for resource in resources:
        create_resource(plan_path, plan, resource)
        print("Created", resource["kind"], resource["metadata"]["name"], flush=True)
    print("Fixture infrastructure created:", plan["namespace"], flush=True)
    print("Next: initialize run-owned Mongo replica set, wait for home seed, create snapshot, pin its UID, then create old API.", flush=True)


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print("Fixture bootstrap failed:", str(error) if isinstance(error, ValueError) else type(error).__name__, file=sys.stderr)
        raise SystemExit(1)
