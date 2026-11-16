"""Disposable kind fixture infrastructure; no production resources."""
import base64
import json
import secrets

NAMESPACE = "cma-migration"
RUNTIME = "docker.io/library/alpine@sha256:" + "0" * 64


def _env_map(values):
    return [{"name": key, "value": value} for key, value in sorted(values.items())]
def mongo_documents(role):
    role_literal = json.dumps(role, separators=(",", ":"))
    return """set -eu
until mongosh --host mongo:27017 --username matrix_admin --password \"$MATRIX_ADMIN_PASSWORD\" --authenticationDatabase admin --quiet --eval 'db.adminCommand({ping:1}).ok' | grep -q 1; do sleep 1; done
mongosh --host mongo:27017 --username matrix_admin --password \"$MATRIX_ADMIN_PASSWORD\" --authenticationDatabase admin --quiet --eval 'try { rs.initiate({_id:"matrix",members:[{_id:0,host:"mongo:27017"}]}) } catch (e) {}'
until mongosh --host mongo:27017 --username matrix_admin --password \"$MATRIX_ADMIN_PASSWORD\" --authenticationDatabase admin --quiet --eval 'try { rs.isMaster().ismaster } catch (e) { false }' | grep -q true; do sleep 1; done
mongosh --host mongo:27017 --username matrix_admin --password \"$MATRIX_ADMIN_PASSWORD\" --authenticationDatabase admin --quiet --eval 'db.getSiblingDB("matrix").runCommand(""" + role_literal + """)'
mongosh --host mongo:27017 --username matrix_admin --password \"$MATRIX_ADMIN_PASSWORD\" --authenticationDatabase admin --quiet --eval 'db.getSiblingDB("matrix").createUser({user: "matrix", pwd: process.env.MATRIX_RUNTIME_PASSWORD, roles: [{role: "cma_am_runtime", db: "matrix"}]})'
mongosh --host mongo:27017 --username matrix --password \"$MATRIX_RUNTIME_PASSWORD\" --authenticationDatabase matrix --quiet --eval 'db.runCommand({ping:1})'
"""
def manifests(api_image, runtime_password, admin_password, encryption_key, mongo_image, role, runtime_image=RUNTIME):
    uri = f"mongodb://matrix:{runtime_password}@mongo.{NAMESPACE}.svc.cluster.local:27017/matrix?authSource=matrix&replicaSet=matrix"
    api_values = {
        "CHRONO_SERVER_BIND": "0.0.0.0:8080",
        "CHRONO_METRICS_BIND": "0.0.0.0:9091",
        "CHRONO_SERVER_PUBLIC_URL": "https://matrix.local",
        "CHRONO_SERVER_CORS_ORIGIN": "https://matrix.local",
        "CHRONO_AUTH_MODE": "trusted",
        "CHRONO_TRUSTED_SUBJECT": "migration-owner",
        "CHRONO_AUTH_ENCRYPTION_KEY": encryption_key,
        "CHRONO_MONGODB_URI": uri,
        "CHRONO_MONGODB_DATABASE": "matrix",
        "CHRONO_DEPLOYMENT_ID": "migration-matrix",
        "CHRONO_SANDBOX_PROVIDER": "agent-sandbox",
        "CHRONO_SANDBOX_BASE_URL": "https://sandbox.matrix.invalid",
        "CHRONO_SANDBOX_API_KEY_FILE": "/etc/cma/agent-sandbox/control-key",
        "CHRONO_RETAIN_OPENSANDBOX": "false",
        "CHRONO_AGENT_SANDBOX_ENABLED": "true",
        "CHRONO_AGENT_SANDBOX_HOME_MODE": "native_vct_empty_v1",
        "CHRONO_AGENT_SANDBOX_HOME_STORAGE_CLASS": "standard",
        "CHRONO_AGENT_SANDBOX_HOME_CAPACITY_GIB": "1",
        "CHRONO_AGENT_SANDBOX_CONTROL_KEY_FILE": "/etc/cma/agent-sandbox/control-key",
        "CHRONO_AGENT_SANDBOX_RUNTIME_TRANSPORT": "direct",
        "CHRONO_SANDBOX_IMAGE": runtime_image,
        "CHRONO_SANDBOX_NAMESPACE": NAMESPACE,
        "CHRONO_NATIVE_PREPARED_TARGET": "0",
        "CHRONO_NATIVE_MACHINE_PREPARED_TARGET": "0",
        "CHRONO_NATIVE_WARM_POOL_TARGET": "0",
        "CHRONO_NATIVE_MACHINE_WARM_POOL_TARGET": "0",
        "CHRONO_USAGE_REPORT_VIEWS_ENABLED": "false",
        "CHRONO_USAGE_REPORT_WARMER_ENABLED": "false",
        "CHRONO_USAGE_ALLOCATION_ROLLUPS_ENABLED": "false",
        "CHRONO_UPSTREAM_BASE_URL": "https://provider.matrix.invalid",
        "CHRONO_UPSTREAM_WIRE_API": "responses",
        "CHRONO_PLATFORM_BASE_URL": "https://platform.matrix.invalid",
    }
    mongo_env = {"MONGO_INITDB_ROOT_USERNAME": "matrix_admin", "MONGO_INITDB_ROOT_PASSWORD": admin_password}
    return [
        {"apiVersion": "apiextensions.k8s.io/v1", "kind": "CustomResourceDefinition",
         "metadata": {"name": "batchsandboxes.sandbox.opensandbox.io"},
         "spec": {"group": "sandbox.opensandbox.io", "scope": "Namespaced",
                  "names": {"plural": "batchsandboxes", "singular": "batchsandbox", "kind": "BatchSandbox"},
                  "versions": [{"name": "v1alpha1", "served": True, "storage": True,
                                "schema": {"openAPIV3Schema": {"type": "object"}}}]}},
        {"apiVersion": "v1", "kind": "Namespace", "metadata": {"name": NAMESPACE}},
        {"apiVersion": "apiextensions.k8s.io/v1", "kind": "CustomResourceDefinition",
         "metadata": {"name": "sandboxes.agents.x-k8s.io"},
         "spec": {"group": "agents.x-k8s.io", "scope": "Namespaced",
                  "names": {"plural": "sandboxes", "singular": "sandbox", "kind": "Sandbox"},
                  "versions": [{"name": "v1beta1", "served": True, "storage": True,
                                "schema": {"openAPIV3Schema": {"type": "object"}}}]}},
        {"apiVersion": "apiextensions.k8s.io/v1", "kind": "CustomResourceDefinition",
         "metadata": {"name": "sandboxwarmpools.extensions.agents.x-k8s.io"},
         "spec": {"group": "extensions.agents.x-k8s.io", "scope": "Namespaced",
                  "names": {"plural": "sandboxwarmpools", "singular": "sandboxwarmpool", "kind": "SandboxWarmPool"},
                  "versions": [{"name": "v1beta1", "served": True, "storage": True,
                                "schema": {"openAPIV3Schema": {"type": "object"}}}]}},
        {"apiVersion": "apiextensions.k8s.io/v1", "kind": "CustomResourceDefinition",
         "metadata": {"name": "sandboxclaims.extensions.agents.x-k8s.io"},
         "spec": {"group": "extensions.agents.x-k8s.io", "scope": "Namespaced",
                  "names": {"plural": "sandboxclaims", "singular": "sandboxclaim", "kind": "SandboxClaim"},
                  "versions": [{"name": "v1beta1", "served": True, "storage": True,
                                "schema": {"openAPIV3Schema": {"type": "object"}}}]}},
        {"apiVersion": "apiextensions.k8s.io/v1", "kind": "CustomResourceDefinition",
         "metadata": {"name": "sandboxtemplates.extensions.agents.x-k8s.io"},
         "spec": {"group": "extensions.agents.x-k8s.io", "scope": "Namespaced",
                  "names": {"plural": "sandboxtemplates", "singular": "sandboxtemplate", "kind": "SandboxTemplate"},
                  "versions": [{"name": "v1beta1", "served": True, "storage": True,
                                "schema": {"openAPIV3Schema": {"type": "object"}}}]}},
        {"apiVersion": "rbac.authorization.k8s.io/v1", "kind": "Role",
         "metadata": {"name": "cma-runtime-sandbox-reclaimer", "namespace": NAMESPACE},
         "rules": [{"apiGroups": [""], "resources": ["pods", "persistentvolumeclaims"],
                    "verbs": ["get", "list", "delete", "patch"]},
                   {"apiGroups": ["agents.x-k8s.io"], "resources": ["sandboxes"],
                    "verbs": ["get", "list", "create", "delete", "patch"]},
                   {"apiGroups": ["extensions.agents.x-k8s.io"],
                    "resources": ["sandboxwarmpools", "sandboxclaims", "sandboxtemplates"],
                    "verbs": ["get", "list", "create", "delete", "patch"]},
                   {"apiGroups": ["sandbox.opensandbox.io"], "resources": ["batchsandboxes"],
                    "verbs": ["get", "list"]}]},
        {"apiVersion": "rbac.authorization.k8s.io/v1", "kind": "RoleBinding",
         "metadata": {"name": "cma-runtime-sandbox-reclaimer", "namespace": NAMESPACE},
         "subjects": [{"kind": "ServiceAccount", "name": "default", "namespace": NAMESPACE}],
         "roleRef": {"apiGroup": "rbac.authorization.k8s.io", "kind": "Role", "name": "cma-runtime-sandbox-reclaimer"}},
        {"apiVersion": "v1", "kind": "ConfigMap", "metadata": {"name": "api-env", "namespace": NAMESPACE},
         "data": api_values},
        {"apiVersion": "v1", "kind": "ConfigMap", "metadata": {"name": "mongo-init", "namespace": NAMESPACE},
         "data": {"init.sh": mongo_documents(role), "keyfile": base64.b64encode(secrets.token_bytes(48)).decode(),
                  "control-key": secrets.token_urlsafe(32)}},
        {"apiVersion": "v1", "kind": "Service", "metadata": {"name": "mongo", "namespace": NAMESPACE},
         "spec": {"selector": {"app": "mongo"}, "ports": [{"name": "mongo", "port": 27017}]}},
        {"apiVersion": "apps/v1", "kind": "Deployment", "metadata": {"name": "mongo", "namespace": NAMESPACE},
         "spec": {"replicas": 1, "selector": {"matchLabels": {"app": "mongo"}}, "template": {
             "metadata": {"labels": {"app": "mongo"}}, "spec": {"containers": [{
                 "name": "mongo", "image": mongo_image, "imagePullPolicy": "Never",
                 "command": ["/bin/sh", "-ec", "cp /init/keyfile /tmp/mongo-keyfile; chmod 400 /tmp/mongo-keyfile; "
                             "chown mongodb:mongodb /tmp/mongo-keyfile; "
                             "exec docker-entrypoint.sh mongod --replSet matrix --bind_ip_all "
                             "--keyFile /tmp/mongo-keyfile --wiredTigerCacheSizeGB 0.25"],
                 "volumeMounts": [{"name": "init", "mountPath": "/init", "readOnly": True}],
                 "env": _env_map(mongo_env), "ports": [{"containerPort": 27017}] }],
                 "volumes": [{"name": "init", "configMap": {"name": "mongo-init"}}]}}}},
        {"apiVersion": "batch/v1", "kind": "Job", "metadata": {"name": "mongo-init", "namespace": NAMESPACE},
         "spec": {"backoffLimit": 20, "template": {"spec": {"restartPolicy": "OnFailure", "containers": [{
             "name": "init", "image": mongo_image, "imagePullPolicy": "Never",
             "command": ["/bin/sh", "/init/init.sh"], "env": [{"name": "MATRIX_ADMIN_PASSWORD", "value": admin_password}, {"name": "MATRIX_RUNTIME_PASSWORD", "value": runtime_password}],
             "volumeMounts": [{"name": "init", "mountPath": "/init"}]}],
             "volumes": [{"name": "init", "configMap": {"name": "mongo-init"}}]}}}},
        {"apiVersion": "v1", "kind": "Service", "metadata": {"name": "api", "namespace": NAMESPACE},
         "spec": {"selector": {"app": "api"}, "ports": [{"name": "http", "port": 8080}]}},
        {"apiVersion": "apps/v1", "kind": "Deployment", "metadata": {"name": "api", "namespace": NAMESPACE},
         "spec": {"replicas": 1, "strategy": {"type": "Recreate"},
                  "selector": {"matchLabels": {"app": "api"}}, "template": {
             "metadata": {"labels": {"app": "api"}}, "spec": {"containers": [{
                 "name": "api", "image": api_image, "imagePullPolicy": "Never", "ports": [{"containerPort": 8080}],
                 "envFrom": [{"configMapRef": {"name": "api-env"}}],
                 "volumeMounts": [{"name": "control-key", "mountPath": "/etc/cma/agent-sandbox", "readOnly": True}],
                 "readinessProbe": {"httpGet": {"path": "/ready", "port": 8080}, "periodSeconds": 2,
                                    "failureThreshold": 60},
                 "livenessProbe": {"httpGet": {"path": "/livez", "port": 8080}, "periodSeconds": 5}}],
                 "volumes": [{"name": "control-key", "configMap": {"name": "mongo-init", "items": [{"key": "control-key", "path": "control-key"}]}}]}}}},
    ]
