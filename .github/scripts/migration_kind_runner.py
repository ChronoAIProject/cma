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

from migration_kind_pods import PodSelectionError, select_ready_api_pod
from migration_kind_manifests import NAMESPACE, RUNTIME, manifests, mongo_documents
SOURCE = re.compile(r"^[a-f0-9]{40}$")
IMAGE = re.compile(r"^(?:[a-z0-9](?:[a-z0-9.-]*[a-z0-9])?(?::[0-9]+)?/)+"
                   r"[a-z0-9][a-z0-9._/-]*@sha256:[a-f0-9]{64}$")


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
    config = row.get("Descriptor", {}).get("annotations", {}).get("config.digest")
    if not isinstance(config, str) or not re.fullmatch(r"sha256:[a-f0-9]{64}", config):
        raise RunnerError("local image metadata lacks an explicit immutable config digest")
    return {"source": source, "image": image, "local_image_id": row.get("Id"), "config": config,
            "labels": {"revision": labels.get("org.opencontainers.image.revision"),
                       "source": labels.get("org.opencontainers.image.source")}}
def checked_dependency(command, image):
    """Validate the disposable linux/amd64 Mongo image before kind load."""
    if not IMAGE.fullmatch(image):
        raise RunnerError("Mongo image must be a full registry digest")
    try:
        rows = json.loads(command(["docker", "image", "inspect", image]))
    except (subprocess.CalledProcessError, json.JSONDecodeError) as error:
        raise RunnerError("Mongo image is not available locally") from error
    if len(rows) != 1 or rows[0].get("Architecture") != "amd64" or rows[0].get("Os") != "linux":
        raise RunnerError("Mongo image must be linux/amd64")
    digests = set(rows[0].get("RepoDigests", []))
    if image not in digests and image.split("@", 1)[1] not in {d.split("@", 1)[-1] for d in digests}:
        raise RunnerError("Mongo image manifest digest is not present in local image metadata")
    return {"image": image, "local_image_id": rows[0].get("Id")}
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
        self.image_aliases = []
        self.plan_path = Path(plan_path)
        self.plan_sha256 = hashlib.sha256(self.plan_path.read_bytes()).hexdigest()
        self.mongo_password = None
        self.mongo_image = None
        self.active_reader = None
        self.diagnostic_errors = []

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

    def alias_image(self, image, role):
        """Give kind a local tag; digest references are normalized by containerd.

        Docker still verifies the immutable digest before this call. The tag is
        unique to this campaign and is removed during cleanup, while the
        evidence continues to bind the exact registry digest.
        """
        alias = f"cma-matrix-{role}-{uuid.uuid4().hex[:12]}"
        self.command(["docker", "tag", image, alias])
        self.image_aliases.append(alias)
        return alias

    def create(self, manifests):
        config = {"kind": "Cluster", "apiVersion": "kind.x-k8s.io/v1alpha4",
                  "nodes": [{"role": "control-plane", "image": "kindest/node:v1.34.0"}]}
        config_path = self.output / "kind.private.json"
        config_path.write_text(json.dumps(config, indent=2))
        self.cluster_created = True
        self.command([self.kind, "create", "cluster", "--name", self.cluster, "--config", str(config_path),
                      "--kubeconfig", str(self.kubeconfig), "--wait", "120s"], timeout=240)
        for image in self.images:
            self.command([self.kind, "load", "docker-image", "--name", self.cluster, image], timeout=300)
        manifest_dir = self.output / "manifests.private"
        manifest_dir.mkdir(exist_ok=True)
        api_manifest = None
        for index, obj in enumerate(manifests):
            if obj['kind'] == 'Deployment' and obj['metadata']['name'] == 'api':
                api_manifest = self.output / 'api-deployment.private.json'
                api_manifest.write_text(json.dumps(obj, indent=2))
                continue
            (manifest_dir / f"{index:02d}-{obj['kind'].lower()}.json").write_text(json.dumps(obj, indent=2))
        self.kubectl(["apply", "-f", str(manifest_dir)])
        self.kubectl(["-n", NAMESPACE, "wait", "--for=condition=complete", "job/mongo-init", "--timeout=180s"], timeout=190)
        self.kubectl(["wait", "--for=condition=Established", "--timeout=45s",
                      "crd/batchsandboxes.sandbox.opensandbox.io",
                      "crd/sandboxes.agents.x-k8s.io",
                      "crd/sandboxwarmpools.extensions.agents.x-k8s.io",
                      "crd/sandboxclaims.extensions.agents.x-k8s.io",
                      "crd/sandboxtemplates.extensions.agents.x-k8s.io"])
        if api_manifest is None:
            raise RunnerError("API deployment missing")
        self.kubectl(["apply", "-f", str(api_manifest)])

    def reader_pods(self):
        return json.loads(self.kubectl(
            ["-n", NAMESPACE, "get", "pods", "-l", "app=api", "-o", "json"]))

    def boot(self, image, source, stage, expected_config):
        started = time.time()
        self.kubectl(["-n", NAMESPACE, "set", "image", "deployment/api", f"api={image}"])
        self.kubectl(["-n", NAMESPACE, "rollout", "status", "deployment/api", "--timeout=180s"], timeout=190)
        selected = select_ready_api_pod(self.reader_pods(), image, expected_config)
        self.active_reader = {"alias": image, "source": source, "config": expected_config,
                              "pod": selected}
        return {"stage": stage, "source": source, "image": image, "serving_pod": selected,
                "started_at": started, "ready_at": time.time()}

    def fixture(self, command, stage, image, source, inventory):
        if not command:
            raise RunnerError("--fixture-command is required for artifact execution")
        active = self.active_reader
        if active is None or active["source"] != source:
            raise RunnerError("fixture has no matching booted reader")
        selected = select_ready_api_pod(self.reader_pods(), active["alias"], active["config"])
        if selected != active["pod"]:
            raise RunnerError("reader changed since boot")
        port = _port()
        mongo_port = _port()
        processes = []
        for target, local, remote in ((f"pod/{selected['name']}", port, 8080), ("service/mongo", mongo_port, 27017)):
            processes.append(subprocess.Popen(
                ["kubectl", "--kubeconfig", str(self.kubeconfig), "--context", f"kind-{self.cluster}",
                 "-n", NAMESPACE, "port-forward", target, f"{local}:{remote}"],
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
            mongo_uri = (
                f"mongodb://matrix:{self.mongo_password}@127.0.0.1:{mongo_port}/matrix"
                "?authSource=matrix&replicaSet=matrix&directConnection=true")
            env = {**os.environ, "MATRIX_STAGE": stage, "MATRIX_API_URL": f"http://127.0.0.1:{port}",
                   "MATRIX_IMAGE": image, "MATRIX_SOURCE": source, "MATRIX_NAMESPACE": NAMESPACE,
                   "MATRIX_PLAN": str(self.plan_path), "MATRIX_PLAN_SHA256": self.plan_sha256,
                   "MATRIX_INVENTORY": str(inventory), "MATRIX_MONGO_URI": mongo_uri,
                   "MATRIX_MONGO_IMAGE": self.mongo_image}
            result = subprocess.run(shlex.split(command), env=env, cwd=self.output, text=True,
                                    stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=self.timeout, check=False)
            log = self.output / f"fixture-{stage}.private.log"
            log.write_text(result.stdout)
            receipt = self.output / "fixture-receipt.json"
            if result.returncode or not receipt.exists():
                raise RunnerError(f"fixture command failed at {stage}")
            after = select_ready_api_pod(self.reader_pods(), active["alias"], active["config"])
            if after != selected:
                raise RunnerError("reader changed during fixture")
            observation = require_fixture_observation(receipt, stage, source, image)
            receipt.rename(self.output / f"fixture-{stage}.json")
            observation["log"] = log.name
            observation["serving_pod"] = selected
            return observation
        finally:
            for process in processes:
                process.terminate()
            for process in processes:
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)

    def diagnostics(self):
        for args in (["get", "pods", "-o", "json"], ["get", "events", "-o", "json"],
                     ["logs", "deployment/mongo", "--tail=120"],
                     ["logs", "job/mongo-init", "--tail=60"],
                     ["logs", "deployment/api", "--tail=120"]):
            try:
                self.kubectl(["-n", NAMESPACE, *args], timeout=20)
            except Exception as error:
                self.diagnostic_errors.append({"argv": args, "error": str(error)})

    def cleanup(self):
        if self.cluster_created:
            self.command([self.kind, "delete", "cluster", "--name", self.cluster, "--kubeconfig", str(self.kubeconfig)], timeout=180)
            clusters = self.command([self.kind, "get", "clusters"], timeout=30).splitlines()
            if self.cluster in clusters:
                raise RunnerError("owned cluster remains after deletion")
            self.cluster_created = False
        for alias in self.image_aliases:
            self.command(["docker", "image", "rm", alias], timeout=30)

def run(args):
    output = Path(args.output)
    output.mkdir(parents=True, exist_ok=False, mode=0o700)
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
    mongo = checked_dependency(command, args.mongo_image)
    campaign = Campaign(output, args.kind, args.timeout, plan_path)
    runtime_password = secrets.token_urlsafe(24)
    admin_password = secrets.token_urlsafe(24)
    campaign.mongo_password = runtime_password
    campaign.mongo_image = args.mongo_image
    key = base64.b64encode(secrets.token_bytes(32)).decode()
    role_output = subprocess.check_output(["docker", "run", "--rm", "--network", "none",
                                           "-e", "CHRONO_MONGODB_DATABASE=matrix",
                                           args.old_image, "--print-runtime-role"], text=True)
    role = json.loads(role_output)
    result = {"schema": "cma-migration-kind-evidence-v1", "environment": "isolated-kind",
              "cluster": campaign.cluster, "old": old, "new": new,
              "mongo": mongo,
              "plan_sha256": campaign.plan_sha256, "started_at": time.time(),
              "boots": [], "fixtures": [], "commands": campaign.commands}
    try:
        old_ref = campaign.alias_image(args.old_image, "old")
        new_ref = campaign.alias_image(args.new_image, "new")
        mongo_ref = campaign.alias_image(args.mongo_image, "mongo")
        campaign.images = [old_ref, new_ref, mongo_ref]
        campaign.create(manifests(old_ref, runtime_password, admin_password, key, mongo_ref, role, args.runtime_image))
        result["boots"].append(campaign.boot(old_ref, args.old_source, "historical-boot", old["config"]))
        result["fixtures"].append(campaign.fixture(args.fixture_command, "old-write", args.old_image, args.old_source, args.inventory))
        result["boots"].append(campaign.boot(new_ref, args.new_source, "current-boot", new["config"]))
        result["fixtures"].append(campaign.fixture(args.fixture_command, "new-read-old-write", args.new_image, args.new_source, args.inventory))
        result["fixtures"].append(campaign.fixture(args.fixture_command, "new-write", args.new_image, args.new_source, args.inventory))
        result["boots"].append(campaign.boot(old_ref, args.old_source, "rollback-boot", old["config"]))
        result["fixtures"].append(campaign.fixture(args.fixture_command, "old-read-new-write", args.old_image, args.old_source, args.inventory))
        result["outcome"] = "UNVERIFIED"
        result["reason"] = "Boot/fixture receipts require full application reader-writer coverage review"
    except Exception as error:
        result["outcome"] = "BLOCKED"
        result["reason"] = str(error)
        campaign.diagnostics()
        raise
    finally:
        try:
            campaign.cleanup()
            result["cleanup"] = {"cluster_absent": True}
        except Exception:
            result["cleanup"] = {"cluster_absent": False}
        result["diagnostic_errors"] = campaign.diagnostic_errors
        result["commands"] = campaign.commands
        result["finished_at"] = time.time()
        (output / "kind-evidence.private.json").write_text(json.dumps(result, indent=2) + "\n")
    return result

def main(argv=None):
    os.umask(0o077)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--old-source", required=True)
    parser.add_argument("--old-image", required=True)
    parser.add_argument("--new-source", required=True)
    parser.add_argument("--new-image", required=True)
    parser.add_argument("--fixture-command")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--plan", type=Path)
    parser.add_argument("--inventory", type=Path,
                        help="source inventory passed to the fixture command")
    parser.add_argument("--kind", default=os.environ.get("KIND_BIN", "kind"))
    parser.add_argument("--mongo-image", default="docker.io/library/mongo@sha256:42545cf0629db918d1a24a2df8bbca6926ed87bfabf59afc85008dd948448cc7")
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
        if not args.fixture_command or not args.output or not args.plan or not args.inventory:
            parser.error("--fixture-command, --output, --plan, and --inventory are required for execution")
        result = run(args)
        print(json.dumps({"outcome": result.get("outcome", "FAIL"), "evidence": str(args.output / "kind-evidence.private.json"),
                          "cluster_cleanup": result.get("cleanup", {}).get("cluster_absent", False)}))
    except (RunnerError, PodSelectionError) as error:
        print(json.dumps({"outcome": "BLOCKED", "reason": str(error)}))
        raise SystemExit(1) from error

if __name__ == "__main__":
    main()
