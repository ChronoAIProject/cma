#!/usr/bin/env python3
"""Run the 12d/13b/12d reader-writer campaign on one disposable kind cluster.

The runner owns only a uniquely named cluster and a synthetic Mongo authority.
It never reads an existing Secret and never uses pod exec. A caller supplies a
fixture command; that command must write and verify the reviewed collection and
document-shape fixtures for the current stage. The runner preserves Mongo while
replacing only the API reader, then records the rollback boot after 13b writes.
"""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import shlex
import socket
import subprocess
import time
import uuid
from urllib.request import urlopen


SOURCE = re.compile(r"^[a-f0-9]{40}$")
IMAGE = re.compile(r"^(?:[a-z0-9](?:[a-z0-9.-]*[a-z0-9])?(?::[0-9]+)?/)+"
                   r"[a-z0-9][a-z0-9._/-]*@sha256:[a-f0-9]{64}$")
NAMESPACE = "cma-migration"
RUNTIME = "docker.io/library/alpine@sha256:" + "0" * 64


class RunnerError(RuntimeError):
    pass


def checked_identity(command, image, source):
    if not SOURCE.fullmatch(source):
        raise RunnerError("source must be a full immutable SHA")
    if not IMAGE.fullmatch(image):
        raise RunnerError("image must be a full registry digest")
    try:
        rows = json.loads(command(["docker", "image", "inspect", image]))
    except (subprocess.CalledProcessError, json.JSONDecodeError) as error:
        raise RunnerError("image is not available locally") from error
    if len(rows) != 1:
        raise RunnerError("image inspect returned an unexpected count")
    row = rows[0]
    labels = row.get("Config", {}).get("Labels", {}) or {}
    if labels.get("org.opencontainers.image.revision") != source:
        raise RunnerError("image source label does not match selected source")
    if row.get("Architecture") != "amd64" or row.get("Os") != "linux":
        raise RunnerError("matrix images must be linux/amd64")
    digests = set(row.get("RepoDigests", []))
    if image not in digests and image.split("@", 1)[1] not in {d.split("@", 1)[-1] for d in digests}:
        raise RunnerError("image manifest digest is not present in local image metadata")
    return {"source": source, "image": image, "config": row.get("Id"),
            "labels": {"revision": labels.get("org.opencontainers.image.revision"),
                       "source": labels.get("org.opencontainers.image.source")}}


def require_fixture_observation(path, stage, source, image):
    """Require a complete fixture receipt, rather than trusting exit status."""
    try:
        value = json.loads(Path(path).read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise RunnerError(f"fixture receipt missing or invalid at {stage}") from error
    required = {"schema", "stage", "source", "image", "collections", "shapes",
                "checks", "samples", "outcome"}
    if set(value) != required or value["schema"] != "cma-migration-fixture-v1":
        raise RunnerError(f"fixture receipt contract mismatch at {stage}")
    if value["stage"] != stage or value["source"] != source or value["image"] != image:
        raise RunnerError(f"fixture receipt identity mismatch at {stage}")
    if not isinstance(value["collections"], list) or not value["collections"]:
        raise RunnerError(f"fixture collection coverage missing at {stage}")
    if not isinstance(value["shapes"], list) or not value["shapes"]:
        raise RunnerError(f"fixture shape coverage missing at {stage}")
    if not isinstance(value["checks"], dict) or not all(value["checks"].values()):
        raise RunnerError(f"fixture checks did not pass at {stage}")
    if not isinstance(value["samples"], int) or value["samples"] < 1:
        raise RunnerError(f"fixture samples missing at {stage}")
    if value["outcome"] != "PASS":
        raise RunnerError(f"fixture outcome was not PASS at {stage}")
    return value


def _env_map(values):
    return [{"name": key, "value": value} for key, value in sorted(values.items())]


def mongo_documents():
    script = """set -eu
until mongosh --host mongo:27017 --username matrix --password "$MATRIX_PASSWORD" --authenticationDatabase admin --quiet --eval 'db.adminCommand({ping:1}).ok' | grep -q 1; do sleep 1; done
mongosh --host mongo:27017 --username matrix --password "$MATRIX_PASSWORD" --authenticationDatabase admin --quiet --eval 'try { rs.initiate({_id:"matrix",members:[{_id:0,host:"mongo:27017"}]}) } catch (e) {}'
until mongosh --host mongo:27017 --username matrix --password "$MATRIX_PASSWORD" --authenticationDatabase admin --quiet --eval 'try { rs.status().ok } catch (e) { 0 }' | grep -q 1; do sleep 1; done
mongosh --host mongo:27017 --quiet --username matrix --password "$MATRIX_PASSWORD" --authenticationDatabase admin --eval 'db.getSiblingDB("matrix").runCommand({ping:1})'
"""
    return script


def manifests(api_image, password, encryption_key, runtime_image=RUNTIME):
    uri = f"mongodb://matrix:{password}@mongo.{NAMESPACE}.svc.cluster.local:27017/matrix?authSource=admin&replicaSet=matrix"
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
        "CHRONO_RETAIN_OPENSANDBOX": "false",
        "CHRONO_AGENT_SANDBOX_ENABLED": "false",
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
    mongo_env = {"MONGO_INITDB_ROOT_USERNAME": "matrix", "MONGO_INITDB_ROOT_PASSWORD": password}
    return [
        {"apiVersion": "v1", "kind": "Namespace", "metadata": {"name": NAMESPACE}},
        {"apiVersion": "v1", "kind": "ConfigMap", "metadata": {"name": "api-env", "namespace": NAMESPACE},
         "data": api_values},
        {"apiVersion": "v1", "kind": "ConfigMap", "metadata": {"name": "mongo-init", "namespace": NAMESPACE},
         "data": {"init.sh": mongo_documents()}},
        {"apiVersion": "v1", "kind": "Service", "metadata": {"name": "mongo", "namespace": NAMESPACE},
         "spec": {"selector": {"app": "mongo"}, "ports": [{"name": "mongo", "port": 27017}]}},
        {"apiVersion": "apps/v1", "kind": "Deployment", "metadata": {"name": "mongo", "namespace": NAMESPACE},
         "spec": {"replicas": 1, "selector": {"matchLabels": {"app": "mongo"}}, "template": {
             "metadata": {"labels": {"app": "mongo"}}, "spec": {"containers": [{
                 "name": "mongo", "image": "mongo:8.0", "imagePullPolicy": "IfNotPresent",
                 "command": ["docker-entrypoint.sh", "mongod", "--replSet", "matrix", "--bind_ip_all"],
                 "env": _env_map(mongo_env), "ports": [{"containerPort": 27017}] }]}}}},
        {"apiVersion": "batch/v1", "kind": "Job", "metadata": {"name": "mongo-init", "namespace": NAMESPACE},
         "spec": {"backoffLimit": 20, "template": {"spec": {"restartPolicy": "OnFailure", "containers": [{
             "name": "init", "image": "mongo:8.0", "imagePullPolicy": "IfNotPresent",
             "command": ["/bin/sh", "/init/init.sh"], "env": [{"name": "MATRIX_PASSWORD", "value": password}],
             "volumeMounts": [{"name": "init", "mountPath": "/init"}]}],
             "volumes": [{"name": "init", "configMap": {"name": "mongo-init"}}]}}}},
        {"apiVersion": "v1", "kind": "Service", "metadata": {"name": "api", "namespace": NAMESPACE},
         "spec": {"selector": {"app": "api"}, "ports": [{"name": "http", "port": 8080}]}},
        {"apiVersion": "apps/v1", "kind": "Deployment", "metadata": {"name": "api", "namespace": NAMESPACE},
         "spec": {"replicas": 1, "selector": {"matchLabels": {"app": "api"}}, "template": {
             "metadata": {"labels": {"app": "api"}}, "spec": {"containers": [{
                 "name": "api", "image": api_image, "imagePullPolicy": "Never", "ports": [{"containerPort": 8080}],
                 "envFrom": [{"configMapRef": {"name": "api-env"}}],
                 "readinessProbe": {"httpGet": {"path": "/ready", "port": 8080}, "periodSeconds": 2,
                                    "failureThreshold": 60},
                 "livenessProbe": {"httpGet": {"path": "/livez", "port": 8080}, "periodSeconds": 5}}]}}}},
    ]


def _port():
    sock = socket.socket()
    sock.bind(("127.0.0.1", 0))
    value = sock.getsockname()[1]
    sock.close()
    return value


class Campaign:
    def __init__(self, output, kind_bin, command_timeout, plan_path):
        self.output = Path(output)
        self.output.mkdir(parents=True, exist_ok=True)
        self.kind = kind_bin
        self.timeout = command_timeout
        self.cluster = "cma-matrix-" + uuid.uuid4().hex[:10]
        self.kubeconfig = self.output / "kubeconfig.private"
        self.cluster_created = False
        self.commands = []
        self.plan_path = Path(plan_path)
        self.plan_sha256 = hashlib.sha256(self.plan_path.read_bytes()).hexdigest()
        self.mongo_uri = None

    def command(self, argv, input_text=None, timeout=None):
        log = self.output / f"command-{len(self.commands):04d}.private.log"
        started = time.time()
        with log.open("wb") as stream:
            proc = subprocess.run(argv, input=input_text, text=True, stdout=stream, stderr=subprocess.STDOUT,
                                  timeout=timeout or self.timeout, check=False)
        self.commands.append({"argv": argv, "exit": proc.returncode, "seconds": time.time() - started,
                              "log": log.name})
        if proc.returncode:
            raise RunnerError(f"command failed: {argv[0]}")
        return log.read_text(errors="replace")

    def kubectl(self, args, **kwargs):
        return self.command(["kubectl", "--kubeconfig", str(self.kubeconfig), "--context", f"kind-{self.cluster}"] + args, **kwargs)

    def create(self, manifests):
        config = {"kind": "Cluster", "apiVersion": "kind.x-k8s.io/v1alpha4",
                  "nodes": [{"role": "control-plane", "image": "kindest/node:v1.34.0"}]}
        config_path = self.output / "kind.private.json"
        config_path.write_text(json.dumps(config, indent=2))
        self.command([self.kind, "create", "cluster", "--name", self.cluster, "--config", str(config_path),
                      "--kubeconfig", str(self.kubeconfig), "--wait", "120s"], timeout=240)
        self.cluster_created = True
        for image in self.images:
            self.command([self.kind, "load", "docker-image", "--name", self.cluster, image], timeout=300)
        manifest_dir = self.output / "manifests.private"
        manifest_dir.mkdir()
        for index, obj in enumerate(manifests):
            (manifest_dir / f"{index:02d}-{obj['kind'].lower()}.json").write_text(json.dumps(obj, indent=2))
        self.kubectl(["apply", "-f", str(manifest_dir)])
        self.kubectl(["-n", NAMESPACE, "wait", "--for=condition=complete", "job/mongo-init", "--timeout=180s"], timeout=190)

    def boot(self, image, source, stage):
        self.kubectl(["-n", NAMESPACE, "set", "image", "deployment/api", f"api={image}"])
        self.kubectl(["-n", NAMESPACE, "rollout", "status", "deployment/api", "--timeout=180s"], timeout=190)
        pods = json.loads(self.kubectl(["-n", NAMESPACE, "get", "pods", "-l", "app=api", "-o", "json"]))
        statuses = []
        for pod in pods.get("items", []):
            container = (pod.get("status", {}).get("containerStatuses") or [{}])[0]
            statuses.append({"phase": pod.get("status", {}).get("phase"),
                             "ready": container.get("ready", False),
                             "restarts": container.get("restartCount", 0),
                             "name": pod.get("metadata", {}).get("name")})
        if not statuses or any(not row["ready"] or row["restarts"] for row in statuses):
            raise RunnerError(f"failed boot at {stage}")
        return {"stage": stage, "source": source, "image": image, "pods": statuses}

    def fixture(self, command, stage, image, source):
        if not command:
            raise RunnerError("--fixture-command is required for artifact execution")
        port = _port()
        mongo_port = _port()
        processes = []
        for service, local, remote in (("api", port, 8080), ("mongo", mongo_port, 27017)):
            processes.append(subprocess.Popen(
                ["kubectl", "--kubeconfig", str(self.kubeconfig), "--context", f"kind-{self.cluster}",
                 "-n", NAMESPACE, "port-forward", f"service/{service}", f"{local}:{remote}"],
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))
        try:
            deadline = time.time() + 30
            while time.time() < deadline:
                try:
                    with urlopen(f"http://127.0.0.1:{port}/ready", timeout=2) as response:
                        if response.status == 200:
                            break
                except Exception:
                    time.sleep(.5)
            else:
                raise RunnerError("API port-forward did not become ready")
            env = {**os.environ, "MATRIX_STAGE": stage, "MATRIX_API_URL": f"http://127.0.0.1:{port}",
                   "MATRIX_IMAGE": image, "MATRIX_SOURCE": source, "MATRIX_NAMESPACE": NAMESPACE,
                   "MATRIX_PLAN": str(self.plan_path), "MATRIX_PLAN_SHA256": self.plan_sha256,
                   "MATRIX_MONGO_URI": self.mongo_uri or ""}
            result = subprocess.run(shlex.split(command), env=env, cwd=self.output, text=True,
                                    stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=self.timeout, check=False)
            log = self.output / f"fixture-{stage}.private.log"
            log.write_text(result.stdout)
            receipt = self.output / "fixture-receipt.json"
            if result.returncode or not receipt.exists():
                raise RunnerError(f"fixture command failed at {stage}")
            observation = require_fixture_observation(receipt, stage, source, image)
            receipt.rename(self.output / f"fixture-{stage}.json")
            observation["log"] = log.name
            return observation
        finally:
            for process in processes:
                process.terminate()
            for process in processes:
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()

    def cleanup(self):
        if self.cluster_created:
            self.command([self.kind, "delete", "cluster", "--name", self.cluster, "--kubeconfig", str(self.kubeconfig)], timeout=180)
            self.cluster_created = False


def run(args):
    output = Path(args.output)
    output.mkdir(parents=True, exist_ok=True)
    command = lambda argv: subprocess.check_output(argv, text=True, stderr=subprocess.PIPE)
    old = checked_identity(command, args.old_image, args.old_source)
    new = checked_identity(command, args.new_image, args.new_source)
    plan_path = Path(args.plan)
    try:
        plan = json.loads(plan_path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise RunnerError("matrix plan is missing or invalid") from error
    if plan.get("schema") != "cma-migration-matrix-plan-v1" or not plan.get("fixtures") or not plan.get("cells"):
        raise RunnerError("matrix plan is incomplete")
    campaign = Campaign(output, args.kind, args.timeout, plan_path)
    campaign.images = [args.old_image, args.new_image]
    password = secrets.token_urlsafe(24)
    campaign.mongo_uri = (
        f"mongodb://matrix:{password}@127.0.0.1:27017/matrix"
        "?authSource=admin&replicaSet=matrix&directConnection=true")
    key = base64.b64encode(secrets.token_bytes(32)).decode()
    result = {"schema": "cma-migration-kind-evidence-v1", "environment": "isolated-kind",
              "cluster": campaign.cluster, "old": old, "new": new,
              "plan_sha256": campaign.plan_sha256, "started_at": time.time(),
              "boots": [], "fixtures": [], "commands": campaign.commands}
    try:
        campaign.create(manifests(args.old_image, password, key, args.runtime_image))
        result["boots"].append(campaign.boot(args.old_image, args.old_source, "historical-boot"))
        result["fixtures"].append(campaign.fixture(args.fixture_command, "old-write", args.old_image, args.old_source))
        result["boots"].append(campaign.boot(args.new_image, args.new_source, "current-boot"))
        result["fixtures"].append(campaign.fixture(args.fixture_command, "new-read-old-write", args.new_image, args.new_source))
        result["fixtures"].append(campaign.fixture(args.fixture_command, "new-write", args.new_image, args.new_source))
        result["boots"].append(campaign.boot(args.old_image, args.old_source, "rollback-boot"))
        result["fixtures"].append(campaign.fixture(args.fixture_command, "old-read-new-write", args.old_image, args.old_source))
        result["outcome"] = "PASS"
    finally:
        try:
            campaign.cleanup()
            result["cleanup"] = {"cluster_absent": True}
        except Exception:
            result["cleanup"] = {"cluster_absent": False}
        result["commands"] = campaign.commands
        result["finished_at"] = time.time()
        (output / "kind-evidence.private.json").write_text(json.dumps(result, indent=2) + "\n")
    return result


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--old-source", required=True)
    parser.add_argument("--old-image", required=True)
    parser.add_argument("--new-source", required=True)
    parser.add_argument("--new-image", required=True)
    parser.add_argument("--fixture-command")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--plan", type=Path)
    parser.add_argument("--kind", default=os.environ.get("KIND_BIN", "kind"))
    parser.add_argument("--runtime-image", default=RUNTIME)
    parser.add_argument("--timeout", type=int, default=600)
    parser.add_argument("--preflight-only", action="store_true",
                        help="validate both local immutable images without creating a cluster")
    args = parser.parse_args(argv)
    try:
        if args.preflight_only:
            command = lambda argv: subprocess.check_output(argv, text=True, stderr=subprocess.PIPE)
            old = checked_identity(command, args.old_image, args.old_source)
            new = checked_identity(command, args.new_image, args.new_source)
            print(json.dumps({"schema": "cma-migration-kind-preflight-v1", "old": old, "new": new}))
            return
        if not args.fixture_command or not args.output or not args.plan:
            parser.error("--fixture-command, --output, and --plan are required for execution")
        result = run(args)
        print(json.dumps({"outcome": result.get("outcome", "FAIL"), "evidence": str(args.output / "kind-evidence.private.json"),
                          "cluster_cleanup": result.get("cleanup", {}).get("cluster_absent", False)}))
    except RunnerError as error:
        print(json.dumps({"outcome": "BLOCKED", "reason": str(error)}))
        raise SystemExit(1) from error


if __name__ == "__main__":
    main()
