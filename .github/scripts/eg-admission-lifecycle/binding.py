"""Project Kubernetes observations without retaining names, addresses or env.

Container instance identity is not proof of process death. The owner must bind
an independent exit/receiver-finality receipt for every departing instance.
"""
import hashlib
import json

from lifecycle import digest, require


def hashed(value):
    require(isinstance(value, str) and bool(value), "missing_runtime_identity")
    return hashlib.sha256(value.encode()).hexdigest()


def project_pods(pods, *, replicas, image_digest, namespace_uid, container_name):
    require(type(replicas) is int and replicas in (1, 2), "invalid_replica_count")
    digest(image_digest)
    require(isinstance(pods, list) and len(pods) == replicas, "unexpected_pod_count")
    rows = []
    for pod in pods:
        metadata, spec, status = pod["metadata"], pod["spec"], pod["status"]
        require(metadata.get("namespace") == "cma-scale", "pod_namespace_mismatch")
        require(not metadata.get("deletionTimestamp"), "terminating_pod")
        require(status.get("phase") == "Running", "pod_not_running")
        require(any(c.get("type") == "Ready" and c.get("status") == "True"
                    for c in status.get("conditions", [])), "pod_not_ready")
        containers = [c for c in status.get("containerStatuses", []) if c.get("name") == container_name]
        require(len(containers) == 1, "gateway_container_missing_or_ambiguous")
        container = containers[0]
        require(container.get("ready") is True, "container_not_ready")
        runtime_image = container.get("imageID", "")
        require(runtime_image.rsplit("sha256:", 1)[-1] == image_digest
                and "sha256:" in runtime_image, "runtime_image_mismatch")
        start = container.get("state", {}).get("running", {}).get("startedAt")
        require(isinstance(start, str) and start.endswith("Z"), "container_start_missing")
        restarts = container.get("restartCount")
        require(type(restarts) is int and restarts >= 0, "invalid_restart_count")
        instance = json.dumps([metadata["uid"], container["containerID"], start, restarts], separators=(",", ":"))
        rows.append({"pod_uid_sha256": hashed(metadata["uid"]),
                     "node_sha256": hashed(spec["nodeName"]),
                     "container_id_sha256": hashed(container["containerID"]),
                     "container_instance_sha256": hashed(instance),
                     "container_started_at": start, "restart_count": restarts,
                     "image_sha256": image_digest})
    require(len({r["pod_uid_sha256"] for r in rows}) == replicas, "duplicate_pod_identity")
    require(len({r["container_instance_sha256"] for r in rows}) == replicas,
            "duplicate_container_identity")
    return {"schema": "eg_lifecycle_pods_v1", "namespace_uid_sha256": hashed(namespace_uid),
            "replicas": replicas, "pods": sorted(rows, key=lambda r: r["pod_uid_sha256"]),
            "distinct_nodes": len({r["node_sha256"] for r in rows}),
            "process_exit_observed": False}
