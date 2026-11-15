#!/usr/bin/env python3
"""One bounded read-only local Kubernetes binding capture; no workload or control."""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import subprocess
import time
from urllib.parse import urlparse

from binding import project_pods
from lifecycle import EvidenceError, immutable_json, require, validate_plan


def capture(plan, kubeconfig, container_name, kubectl, execute=subprocess.run, record=lambda row: None):
    plan = validate_plan(plan)
    require(Path(kubeconfig).is_absolute() and Path(kubectl).is_absolute(), "explicit_tool_paths_required")
    base = [kubectl, "--kubeconfig", kubeconfig, "--context", "kind-cma-scale",
            "--namespace", "cma-scale", "--request-timeout=10s"]
    started = time.monotonic()
    calls = []

    def read(arguments):
        ordinal = len(calls)
        record({"ordinal": ordinal, "boundary": "start", "elapsed_ms": (time.monotonic() - started) * 1000})
        try:
            result = execute(base + arguments, capture_output=True, text=True, timeout=15, check=False)
        except (OSError, subprocess.TimeoutExpired):
            record({"ordinal": ordinal, "boundary": "end", "status": "read_incomplete",
                    "elapsed_ms": (time.monotonic() - started) * 1000})
            raise
        observation = {"ordinal": ordinal, "boundary": "end",
                       "elapsed_ms": (time.monotonic() - started) * 1000,
                       "status": "ok" if result.returncode == 0 else "read_failed"}
        if result.returncode == 0:
            observation["output_sha256"] = hashlib.sha256(result.stdout.encode()).hexdigest()
        record(observation)
        require(result.returncode == 0, "kubernetes_read_failed")
        calls.append(observation)
        return result.stdout

    server = read(["config", "view", "--minify", "-o", "jsonpath={.clusters[0].cluster.server}"]).strip()
    url = urlparse(server)
    require(url.scheme == "https" and url.hostname in ("127.0.0.1", "::1", "localhost")
            and not url.username and not url.password and not url.query and not url.fragment,
            "nonlocal_cluster_refused")
    namespace = read(["get", "namespace", "cma-scale", "-o", "jsonpath={.metadata.uid}"]).strip()
    require(hashlib.sha256(namespace.encode()).hexdigest() == plan["bindings"]["namespace"],
            "namespace_identity_changed")
    labels = json.loads(read(["get", "deployment", "cma-event-gateway", "-o",
                             "jsonpath={.spec.selector.matchLabels}"]))
    require(isinstance(labels, dict) and bool(labels)
            and all(isinstance(k, str) and isinstance(v, str) for k, v in labels.items()),
            "deployment_selector_missing")
    selector = ",".join(k + "=" + v for k, v in sorted(labels.items()))
    # This projection excludes Pod env, volumes, commands and secret references.
    projection = ('{range .items[*]}{.metadata.uid}{"\\t"}{.metadata.namespace}{"\\t"}'
                  '{.metadata.deletionTimestamp}{"\\t"}{.spec.nodeName}{"\\t"}'
                  '{.status.phase}{"\\t"}{.status.conditions}{"\\t"}'
                  '{.status.containerStatuses}{"\\n"}{end}')
    rows = read(["get", "pods", "-l", selector, "-o", "jsonpath=" + projection])
    pods = []
    for line in rows.splitlines():
        fields = line.split("\t")
        require(len(fields) == 7, "invalid_pod_projection")
        uid, ns, deletion, node, phase, conditions, containers = fields
        pods.append({"metadata": {"uid": uid, "namespace": ns, "deletionTimestamp": deletion},
                     "spec": {"nodeName": node}, "status": {"phase": phase,
                     "conditions": json.loads(conditions), "containerStatuses": json.loads(containers)}})
    result = project_pods(pods, replicas=plan["replicas"], namespace_uid=namespace,
                          image_digest=plan["bindings"]["application_image"], container_name=container_name)
    # Recheck namespace after collection. A replace/recreate invalidates the snapshot.
    after = read(["get", "namespace", "cma-scale", "-o", "jsonpath={.metadata.uid}"]).strip()
    require(after == namespace, "namespace_changed_during_capture")
    result.update(run_id=plan["run_id"], cohort_id=plan["cohort_id"],
                  recorded_at_utc=datetime.datetime.now(datetime.timezone.utc).isoformat(),
                  capture_elapsed_ms=(time.monotonic() - started) * 1000,
                  api_server_sha256=hashlib.sha256(server.encode()).hexdigest(),
                  kubeconfig_path_sha256=hashlib.sha256(kubeconfig.encode()).hexdigest(),
                  observations=calls, read_only=True,
                  scope="Container status snapshot; no endpoint test, image-source attestation or process death proof.")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--plan", required=True)
    parser.add_argument("--kubeconfig", required=True)
    parser.add_argument("--kubectl", required=True)
    parser.add_argument("--container", required=True)
    parser.add_argument("--out", required=True)
    args = parser.parse_args()
    try:
        plan = validate_plan(json.loads(Path(args.plan).read_text()))
        destination = Path(args.out)
        require(not destination.exists(), "output_already_exists")
        observations = destination.with_suffix(destination.suffix + ".observations.jsonl")
        fd = os.open(observations, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "w") as journal:
            def record(row):
                journal.write(json.dumps(row, separators=(",", ":")) + "\n")
                journal.flush()
                os.fsync(journal.fileno())
            try:
                result = capture(plan, args.kubeconfig, args.container, args.kubectl, record=record)
            except (EvidenceError, OSError, ValueError, KeyError, TypeError, subprocess.TimeoutExpired) as error:
                result = {"schema": "eg_lifecycle_capture_failure_v1", "status": "BLOCKED",
                          "run_id": plan["run_id"], "cohort_id": plan["cohort_id"],
                          "code": str(error) if isinstance(error, EvidenceError) else "capture_failed"}
        result["observation_journal_sha256"] = hashlib.sha256(observations.read_bytes()).hexdigest()
        checksum = immutable_json(destination, result)
    except EvidenceError as error:
        print(json.dumps({"status": "BLOCKED", "code": str(error)}))
        return 2
    except (OSError, ValueError, KeyError, TypeError, subprocess.TimeoutExpired):
        print(json.dumps({"status": "BLOCKED", "code": "capture_failed"}))
        return 2
    failed = result.get("status") == "BLOCKED"
    print(json.dumps({"status": "BLOCKED" if failed else "CAPTURED", "sha256": checksum, "acceptance": False}))
    return 2 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
