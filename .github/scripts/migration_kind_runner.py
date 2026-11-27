#!/usr/bin/env python3
"""Run the 12d/13b/12d reader-writer campaign on one disposable kind cluster.

The runner owns only a uniquely named cluster and synthetic Mongo authority.
It never reads an existing Secret or uses pod exec. The supplied fixture must
write and verify reviewed shapes for each stage. The runner preserves Mongo while
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
from migration_kind_manifests import IDENTITY, NAMESPACE, RUNTIME, manifests, mongo_documents
from migration_redaction import EvidenceRedactor
from migration_boot_observation import observe_boot
from migration_image_preflight import (
    ImagePreflight, RunnerError, checked_identity, checked_dependency, checked_identity_dependency,
)
import migration_catalog as catalog
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
# Retained default for local campaigns; CI passes a digest-pinned node image.
KIND_NODE = "kindest/node:v1.34.0"
KIND_NODE_PINNED = re.compile(r"^(?:docker\.io/)?kindest/node(?::v[0-9]+\.[0-9]+\.[0-9]+)?@sha256:[a-f0-9]{64}$")


class Campaign:
    def __init__(self, output, kind_bin, command_timeout, plan_path, node_image=KIND_NODE):
        if node_image != KIND_NODE and not KIND_NODE_PINNED.fullmatch(node_image):
            raise RunnerError("kind node image must be pinned by digest")
        self.node_image = node_image
        self.image_store = None
        self.output = Path(output)
        self.output.mkdir(parents=True, exist_ok=True)
        self.kind = kind_bin
        self.timeout = command_timeout
        self.cluster = "cma-matrix-" + uuid.uuid4().hex[:10]
        self.kubeconfig = self.output / "kubeconfig.private"
        self.cluster_created = False
        self.commands = []
        self.redactor = EvidenceRedactor()
        self.image_aliases = []
        self.plan_path = Path(plan_path)
        self.plan_sha256 = hashlib.sha256(self.plan_path.read_bytes()).hexdigest()
        self.mongo_password = None
        self.mongo_image = None
        self.active_reader = None
        self.pending_boot = None
        self.diagnostic_errors = []
        self.identity_image = None
        self.admin_bearer = ""
        self.auth_state = self.output / "browser-custody.ephemeral.private"
        self.auth_state.unlink(missing_ok=True)
        self.auth_state.touch(mode=0o600)

    def command(self, argv, input_text=None, timeout=None):
        log = self.output / f"command-{len(self.commands):04d}.private.log"
        started = time.time()
        try:
            proc = subprocess.run(argv, input=input_text, text=True, encoding='utf-8', errors='replace',
                                  stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                  timeout=timeout or self.timeout, check=False)
        except subprocess.TimeoutExpired as error:
            log.write_text(self.redactor.text(error.stdout or ''))
            self.commands.append({"argv": [self.redactor.text(arg) for arg in argv],
                                  "exit": None, "timed_out": True,
                                  "seconds": time.time() - started, "log": log.name})
            raise RunnerError(f"command timed out: {self.redactor.text(argv[0])}") from None
        log.write_text(self.redactor.text(proc.stdout))
        self.commands.append({"argv": [self.redactor.text(arg) for arg in argv],
                              "exit": proc.returncode, "seconds": time.time() - started,
                              "log": log.name})
        if proc.returncode:
            raise RunnerError(f"command failed: {self.redactor.text(argv[0])}")
        # Readers need the original JSON. Only retained evidence is redacted.
        return proc.stdout

    def containerd_store(self):
        """Whether Docker keeps images in its containerd store (#1327).

        That store cannot `kind load docker-image` a multi-platform image such
        as the Mongo and Python dependencies, so it exports only linux/amd64.
        """
        if self.image_store is None:
            status = self.command(["docker", "info", "--format", "{{json .DriverStatus}}"], timeout=30)
            self.image_store = "containerd" if "io.containerd.snapshotter" in status else "classic"
        return self.image_store == "containerd"

    def load_image(self, image):
        if not self.containerd_store():
            self.command([self.kind, "load", "docker-image", "--name", self.cluster, image], timeout=300)
            return
        archive = self.output / f"image-{len(self.commands):04d}.private.tar"
        try:
            self.command(["docker", "save", "--platform", "linux/amd64", "--output", str(archive), image],
                         timeout=300)
            self.command([self.kind, "load", "image-archive", "--name", self.cluster, str(archive)],
                         timeout=300)
        finally:
            archive.unlink(missing_ok=True)

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
        self.redactor.collect(manifests)
        config = {"kind": "Cluster", "apiVersion": "kind.x-k8s.io/v1alpha4",
                  "nodes": [{"role": "control-plane", "image": self.node_image}]}
        config_path = self.output / "kind.private.json"
        config_path.write_text(json.dumps(config, indent=2))
        self.cluster_created = True
        self.command([self.kind, "create", "cluster", "--name", self.cluster, "--config", str(config_path),
                      "--kubeconfig", str(self.kubeconfig), "--wait", "120s"], timeout=240)
        for image in self.images:
            self.load_image(image)
        infrastructure = []
        api_manifest = None
        for obj in manifests:
            if obj['kind'] == 'Deployment' and obj['metadata']['name'] == 'api':
                api_manifest = obj
                continue
            infrastructure.append(obj)
        # Synthetic authentication material is needed only while the owned
        # cluster lives. Stream it to kubectl; evidence must not retain it in
        # manifest files. command() records argv/output, never this stdin.
        self.kubectl(["apply", "-f", "-"], input_text=json.dumps(
            {"apiVersion": "v1", "kind": "List", "items": infrastructure}))
        self.kubectl(["-n", NAMESPACE, "wait", "--for=condition=complete", "job/mongo-init", "--timeout=180s"], timeout=190)
        if self.identity_image:
            self.kubectl(["-n", NAMESPACE, "wait", "--for=condition=available",
                          "deployment/identity", "--timeout=90s"], timeout=100)
        self.kubectl(["wait", "--for=condition=Established", "--timeout=45s",
                      "crd/batchsandboxes.sandbox.opensandbox.io",
                      "crd/sandboxes.agents.x-k8s.io",
                      "crd/sandboxwarmpools.extensions.agents.x-k8s.io",
                      "crd/sandboxclaims.extensions.agents.x-k8s.io",
                      "crd/sandboxtemplates.extensions.agents.x-k8s.io"])
        if api_manifest is None:
            raise RunnerError("API deployment missing")
        # Capture history before the first API pod can exist, including an
        # attempt that fails and is replaced before rollout status returns.
        self.pending_boot = {"pods": self.reader_pods(), "started_at": time.time()}
        self.kubectl(["apply", "-f", "-"], input_text=json.dumps(api_manifest))

    def reader_pods(self):
        # kubectl's client-generated List can discard resourceVersion, which
        # would make replaying every startup attempt impossible. Keep the
        # API server's PodList envelope for the subsequent bounded watch.
        return json.loads(self.kubectl(
            ["--request-timeout=15s", "get", "--raw",
             f"/api/v1/namespaces/{NAMESPACE}/pods?labelSelector=app%3Dapi"], timeout=20))

    def boot(self, image, source, stage, expected_config):
        baseline = self.pending_boot or {"pods": self.reader_pods(), "started_at": time.time()}
        self.pending_boot = None
        self.kubectl(["-n", NAMESPACE, "set", "image", "deployment/api", f"api={image}"])
        self.kubectl(["-n", NAMESPACE, "rollout", "status", "deployment/api", "--timeout=180s"], timeout=190)
        selected = select_ready_api_pod(self.reader_pods(), image, expected_config)
        ready_at = time.time()
        observation = observe_boot(self.kubectl, NAMESPACE, baseline["pods"],
                                   selected, image, expected_config)
        self.active_reader = {"alias": image, "source": source, "config": expected_config,
                              "pod": selected}
        return {"stage": stage, "source": source, "image": image, "serving_pod": selected,
                "started_at": baseline["started_at"], "ready_at": ready_at,
                "startup_observation": observation}

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
        identity_port = _port() if self.identity_image else None
        processes = []
        targets = [(f"pod/{selected['name']}", port, 8080), ("service/mongo", mongo_port, 27017)]
        if identity_port:
            targets.append(("service/identity", identity_port, 8080))
        for target, local, remote in targets:
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
                   "MATRIX_READER_POD_UID": selected["uid"],
                   "MATRIX_PLAN": str(self.plan_path), "MATRIX_PLAN_SHA256": self.plan_sha256,
                   "MATRIX_INVENTORY": str(inventory), "MATRIX_MONGO_URI": mongo_uri,
                   "MATRIX_MONGO_IMAGE": self.mongo_image,
                   "MATRIX_ADMIN_BEARER": self.admin_bearer,
                   "MATRIX_AUTH_STATE_FILE": str(self.auth_state),
                   "MATRIX_IDENTITY_URL": "" if identity_port is None else f"http://127.0.0.1:{identity_port}"}
            result = subprocess.run(shlex.split(command), env=env, cwd=self.output, text=True,
                                    stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=self.timeout, check=False)
            log = self.output / f"fixture-{stage}.private.log"
            log.write_text(self.redactor.text(result.stdout))
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
                     ["logs", "deployment/api", "--tail=120"],
                     ["logs", "deployment/identity", "--tail=120"] if self.identity_image else []):
            if not args:
                continue
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
        self.auth_state.unlink(missing_ok=True)

def run(args):
    output = Path(args.output)
    output.mkdir(parents=True, exist_ok=False, mode=0o700)
    command = ImagePreflight(output, lambda argv: subprocess.check_output(
        argv, text=True, stderr=subprocess.PIPE))
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
    identity = checked_identity_dependency(command, args.identity_image)
    campaign = Campaign(output, args.kind, args.timeout, plan_path,
                        getattr(args, "kind_node_image", KIND_NODE))
    runtime_password = secrets.token_urlsafe(24)
    admin_password = secrets.token_urlsafe(24)
    campaign.mongo_password = runtime_password
    campaign.mongo_image = args.mongo_image
    key = base64.b64encode(secrets.token_bytes(32)).decode()
    from migration_fixture_dependencies import runtime_role
    role = runtime_role(subprocess.check_output, args)
    identity_credentials = {
        "client_id": "migration-local-client-" + uuid.uuid4().hex,
        "client_secret": secrets.token_urlsafe(32),
        "bearer": "migration-bearer-" + secrets.token_urlsafe(32),
        "admin_bearer": "migration-admin-bearer-" + secrets.token_urlsafe(32),
        "outsider_bearer": "migration-outsider-bearer-" + secrets.token_urlsafe(32),
        "refresh_token": "migration-refresh-" + secrets.token_urlsafe(32),
        "expires_at": int(time.time()) + 3600,
    }
    campaign.admin_bearer = identity_credentials["admin_bearer"]
    result = {"schema": "cma-migration-kind-evidence-v1", "environment": "isolated-kind",
              "cluster": campaign.cluster, "old": old, "new": new,
              "mongo": mongo, "identity": identity,
              "plan_sha256": campaign.plan_sha256, "started_at": time.time(),
              "boots": [], "fixtures": [], "catalogs": [], "catalog_comparisons": [],
              "commands": campaign.commands}
    try:
        old_ref = campaign.alias_image(args.old_image, "old")
        new_ref = campaign.alias_image(args.new_image, "new")
        mongo_ref = campaign.alias_image(args.mongo_image, "mongo")
        identity_ref = campaign.alias_image(args.identity_image, "identity")
        campaign.identity_image = identity_ref
        campaign.images = [old_ref, new_ref, mongo_ref, identity_ref]
        objects = manifests(old_ref, runtime_password, admin_password, key, mongo_ref, role,
                            args.runtime_image, identity_ref, identity_credentials)
        from migration_fixture_dependencies import configure, capture
        objects = configure(objects, args)
        campaign.create(catalog.infrastructure(objects, NAMESPACE, secrets.token_urlsafe(24)))
        previous_catalog = None
        stages = [("old-write", "historical-boot", old_ref, old),
                  ("new-read-old-write", "current-boot", new_ref, new),
                  ("new-write", None, new_ref, new),
                  ("old-read-new-write", "rollback-boot", old_ref, old)]
        for stage, boot, alias, artifact in stages:
            source, image = artifact["source"], artifact["image"]
            if boot:
                result["boots"].append(campaign.boot(alias, source, boot, artifact["config"]))
            result["fixtures"].append(campaign.fixture(args.fixture_command, stage, image, source, args.inventory))
            selected = campaign.active_reader["pod"]
            if select_ready_api_pod(campaign.reader_pods(), alias, artifact["config"]) != selected:
                raise RunnerError("reader changed before catalog capture")
            receipt, snapshot = catalog.capture(campaign, NAMESPACE, mongo_ref, stage, source, image)
            if select_ready_api_pod(campaign.reader_pods(), alias, artifact["config"]) != selected:
                raise RunnerError("reader changed during catalog capture")
            receipt["serving_pod"] = selected
            result["catalogs"].append(receipt)
            documents = capture(campaign, args, NAMESPACE, mongo_ref, stage, source, image)
            if documents is not None:
                if select_ready_api_pod(campaign.reader_pods(), alias, artifact["config"]) != selected:
                    raise RunnerError("reader changed during document capture")
                result.setdefault("document_snapshots", []).append({**documents, "serving_pod": selected})
            if previous_catalog is not None:
                comparison = catalog.compare(previous_catalog, snapshot)
                result["catalog_comparisons"].append({"to_stage": stage, **comparison})
                if comparison["outcome"] != "PRESERVED":
                    raise RunnerError("catalog changed across reader/fixture stage")
            previous_catalog = snapshot
        result["outcome"] = "UNVERIFIED"
        result["reason"] = "Boot/fixture receipts require full application reader-writer coverage review"
    except (KeyboardInterrupt, SystemExit) as error:
        result["outcome"] = "CANCELLED"
        result["reason"] = f"campaign interrupted: {type(error).__name__}"
        raise
    except Exception as error:
        result["outcome"] = "BLOCKED"
        result["reason"] = str(error)
        campaign.diagnostics()
        raise
    finally:
        try:
            campaign.cleanup()
            result["cleanup"] = {"cluster_absent": True}
        except Exception as error:
            result["cleanup"] = {"cluster_absent": False, "error": str(error)}
        result["diagnostic_errors"] = campaign.diagnostic_errors
        result["commands"] = campaign.commands
        result["finished_at"] = time.time()
        (output / "kind-evidence.private.json").write_text(
            campaign.redactor.text(json.dumps(result, indent=2)) + "\n")
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
    parser.add_argument("--kind-node-image", default=KIND_NODE,
                        help="digest-pinned kindest/node image; the default keeps the local tag")
    parser.add_argument("--mongo-image", default="docker.io/library/mongo@sha256:42545cf0629db918d1a24a2df8bbca6926ed87bfabf59afc85008dd948448cc7")
    parser.add_argument("--runtime-image", default=RUNTIME)
    parser.add_argument("--identity-image", default=IDENTITY)
    from migration_fixture_dependencies import add_arguments
    add_arguments(parser)
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
