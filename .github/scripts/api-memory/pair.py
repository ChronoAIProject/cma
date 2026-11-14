#!/usr/bin/env python3
"""Owner-scheduled sequential API image comparison. No build or production access."""
import argparse
import base64
import datetime
import json
import os
from pathlib import Path
import secrets
import signal
import subprocess
import time

import repeat
import seed

SOURCE = "67b3fbc5df5f09fcecd9ddbbba723ae010df9cc6"
OLD = "cma-api-batch9-native:amd64-67b3fbc5df5f"
NEW = "cma-api-batch9r-official:67b3fbc5df5f"
MONGO = "mongo:8.0.13@sha256:cf340b1e5283843c63eb12999922f20c463ae31285f746d30f05dcc21cd1d47c"
LABEL = seed.LABEL


def utc():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def private_write(path, text):
    with open(path, "x", opener=lambda name, flags: os.open(name, flags, 0o600)) as stream:
        stream.write(text)


def docker(*args, timeout=30, input=None):
    return seed.docker(*args, timeout=timeout, input=input)


def image(reference):
    # Only fixed local incident tags; no pulls or registry operations.
    value = json.loads(docker("image", "inspect", "--format",
                            '{"id":{{json .Id}},"descriptor":{{json .Descriptor}},"architecture":{{json .Architecture}},"source":{{json (index .Config.Labels "org.opencontainers.image.revision")}}}', reference))
    if value["architecture"] != "amd64" or value["source"] != SOURCE:
        raise ValueError("local image source/architecture mismatch")
    if reference == OLD:
        descriptor = value["descriptor"]
        if (descriptor["digest"] != "sha256:322f08a15db3026560193e22eae6c590549366c2758edae2dff9b22be8ffa9e8"
                or descriptor["annotations"]["config.digest"] != "sha256:599ca9467f6813bcdf7146f5dc173787fe2aaaa5e672e1bb85f75124e86e3c33"):
            raise ValueError("old control is not the retained shipped manifest/config")
    elif reference == NEW:
        descriptor = value["descriptor"]
        if (descriptor["digest"] != "sha256:8a09ee166cd7ec8a850fadef00b5beae8d62aa742e6080644f7e7c54813a2540"
                or descriptor["annotations"]["config.digest"] != "sha256:2a8c21da475b6b3d9d4518dca3b865e88db03697bffdb22e602a1b802e1d6686"):
            raise ValueError("corrected image differs from the build owner's qualified manifest/config")
    return value


def remove_owned(container, owner):
    label = docker("inspect", "--format", '{{index .Config.Labels "' + LABEL + '"}}', container).strip()
    if label != owner:
        raise RuntimeError("refusing cleanup of foreign container")
    docker("rm", "-f", "-v", container)


def wait_primary(container):
    for _ in range(60):
        try:
            result = docker("exec", container, "mongosh", "--quiet", "--eval",
                            "if (!db.hello().isWritablePrimary) quit(2); print('ready')", timeout=5)
            if result.strip() == "ready":
                return
        except (subprocess.SubprocessError, RuntimeError):
            pass
        time.sleep(1)
    raise RuntimeError("disposable Mongo primary unavailable")


def startup(config, output):
    rows = []
    deadline = time.monotonic() + 120
    try:
        while time.monotonic() < deadline:
            row = {"utc": utc(), "outcome": "failed"}
            try:
                status = docker("exec", config["container"], "cat", "/proc/1/status", timeout=5)
                stat = docker("exec", config["container"], "cat", "/proc/1/stat", timeout=5)
                row["process"] = repeat.process_fields(status, stat)
            except Exception as error:
                row["process_observation_failure"] = type(error).__name__
            try:
                repeat.get(config["origin"], "/ready")
                repeat.get(config["origin"], "/openapi.json")
                row["outcome"] = "ready"
                rows.append(row)
                return
            except Exception as error:
                row["failure_type"] = type(error).__name__
                rows.append(row)
                time.sleep(1)
        raise RuntimeError("actual API HTTP startup failed")
    finally:
        (output / "startup.json").write_text(json.dumps(rows, indent=2) + "\n")


def environment(subject, mongo, database, key):
    return {
        "CHRONO_AUTH_MODE": "trusted", "CHRONO_TRUSTED_SUBJECT": subject,
        "CHRONO_MONGODB_URI": f"mongodb://{mongo}:27017/?replicaSet=cma_memory_rca",
        "CHRONO_MONGODB_DATABASE": database,
        "CHRONO_AUTH_ENCRYPTION_KEY": key,
        "CHRONO_SERVER_BIND": "0.0.0.0:8080", "CHRONO_METRICS_BIND": "0.0.0.0:9091",
        "CHRONO_SERVER_PUBLIC_URL": "http://127.0.0.1:18980",
        "CHRONO_SERVER_CORS_ORIGIN": "http://127.0.0.1:18980",
        "CHRONO_SANDBOX_BASE_URL": "http://127.0.0.1:9",
        "CHRONO_SANDBOX_API_KEY_FILE": "/run/synthetic-provider-key",
        "CHRONO_SANDBOX_IMAGE": "synthetic.invalid/runtime@sha256:" + "a" * 64,
        "CHRONO_SANDBOX_NAMESPACE": "memory-rca",
        "CHRONO_UPSTREAM_BASE_URL": "https://synthetic.invalid/api/v1/proxy/s/model",
        "CHRONO_UPSTREAM_WIRE_API": "responses", "CHRONO_PLATFORM_BASE_URL": "https://synthetic.invalid",
        "CHRONO_DEPLOYMENT_ID": "cma-memory-rca",
        "CHRONO_NATIVE_PREPARED_TARGET": "0", "CHRONO_NATIVE_MACHINE_PREPARED_TARGET": "0",
        "CHRONO_NATIVE_WARM_POOL_TARGET": "0", "CHRONO_NATIVE_MACHINE_WARM_POOL_TARGET": "0",
        "CHRONO_AGENT_MANAGER_REPLICAS_ENABLED": "false", "TOKIO_WORKER_THREADS": "8", "RUST_LOG": "info",
    }


def fixture_status(mongo, database):
    script = "const target = db.getSiblingDB(" + json.dumps(database) + r''');
const agent = target.agents.findOne({});
const counts = Object.fromEntries(['sandboxes', 'workspaces', 'worktrees', 'agents', 'agent_log'].map(k => [k, target[k].countDocuments({})]));
const pending = ['sandboxes', 'workspaces', 'agents'].reduce((n,k) => n + target[k].countDocuments({work_pending: true}), 0);
print(JSON.stringify({counts, pending_work: pending, head: Number(agent.record.conversation.last_seq), stopped: agent.status === 'stopped'}));
'''
    status = json.loads(docker("exec", mongo, "mongosh", "--quiet", "--eval", script))
    if (status["counts"] != {"sandboxes": 1, "workspaces": 1, "worktrees": 1, "agents": 1, "agent_log": 515}
            or status["head"] != 515 or not status["stopped"] or status["pending_work"]):
        raise RuntimeError("fixed dataset changed or acquired background work")
    return status


def run_variant(kind, binding, fixture, folder, owner, mongo, private, key):
    folder.mkdir()
    container = owner + "-" + kind
    database = "cma_memory_rca_" + owner.removeprefix("cma-memory-rca-") + "_" + kind
    created = False
    result = {"variant": kind, "started_utc": utc(), "image": binding, "outcome": "incomplete"}
    try:
        seed.seed(fixture, "mongodb://127.0.0.1:18917", database, mongo)
        result["dataset_before"] = fixture_status(mongo, database)
        subject = repeat.private_json(fixture)["subject"]
        settings = environment(subject, mongo, database, key)
        envfile = private / (kind + ".env")
        private_write(envfile, "".join(f"{name}={value}\n" for name, value in settings.items()))
        config = {"environment": "isolated-local", "synthetic_identity": True, "source": SOURCE,
                  "origin": "http://127.0.0.1:18980", "metrics_origin": "http://127.0.0.1:18991",
                  "container": container, "docker_image_id": binding["id"], "dataset": str(fixture)}
        private_write(private / (kind + ".json"), json.dumps(config, indent=2))
        docker("create", "--name", container, "--label", f"{LABEL}={owner}", "--network", owner,
               "--platform", "linux/amd64", "--cpus", "2", "--cpuset-cpus", "0-7",
               "--memory", "2g", "--memory-swap", "2g", "--pids-limit", "256",
               "--env-file", str(envfile), "--publish", "127.0.0.1:18980:8080",
               "--publish", "127.0.0.1:18991:9091", "--mount",
               f"type=bind,source={private / 'provider-key'},destination=/run/synthetic-provider-key,readonly",
               binding["id"])
        created = True
        docker("start", container)
        result["container_started_utc"] = utc()
        startup(config, folder)
        # First runtime metrics emit every ten seconds. Retain the wait explicitly.
        result["metrics_wait_seconds"] = 12
        time.sleep(12)
        repeat.run(config, folder / "measurement")
        result["dataset_after"] = fixture_status(mongo, database)
        result["outcome"] = "completed"
    except Exception as error:
        result["failure_type"] = type(error).__name__
    finally:
        if created:
            try:
                result["terminal_container_state"] = json.loads(docker("inspect", "--format",
                    '{"running":{{.State.Running}},"oom_killed":{{.State.OOMKilled}},"exit_code":{{.State.ExitCode}},"restart_count":{{.RestartCount}}}', container))
                # API logs remain private, may contain the synthetic subject. No payload copied to report.
                try:
                    private_write(private / (kind + ".log"), docker("logs", container, timeout=10))
                except Exception as error:
                    result["private_log_failure_type"] = type(error).__name__
            finally:
                remove_owned(container, owner)
        result["ended_utc"] = utc()
        (folder / "variant.json").write_text(json.dumps(result, indent=2) + "\n")
    return result


def run(fixture, output):
    fixture = fixture.resolve()
    repeat.private_json(fixture)
    output.mkdir(mode=0o700)
    private = output / "private"
    private.mkdir(mode=0o700)
    private_write(private / "provider-key", secrets.token_urlsafe(32))
    # Parent stays 0700 on host; the nonroot container can read the explicitly mounted synthetic file.
    (private / "provider-key").chmod(0o444)
    key = base64.b64encode(secrets.token_bytes(32)).decode()
    owner = "cma-memory-rca-" + secrets.token_hex(6)
    mongo = owner + "-mongo"
    summary = {"started_utc": utc(), "source": SOURCE, "owner": owner, "variants": [],
               "predeclared_order": ["musl", "zig"],
               "order_decision": "RCA lead 2026-11-14 23:40 SGT, before any runtime; protect corrected-image qualification deadline"}
    network_created = mongo_created = False
    try:
        bindings = [("musl", image(NEW)), ("zig", image(OLD))]
        docker("network", "create", "--internal", "--label", f"{LABEL}={owner}", owner)
        network_created = True
        docker("run", "--detach", "--pull", "never", "--name", mongo, "--hostname", mongo,
               "--label", f"{LABEL}={owner}", "--network", owner, "--cpus", "0.5",
               "--memory", "768m", "--memory-swap", "768m", "--pids-limit", "128",
               "--publish", "127.0.0.1:18917:27017", MONGO,
               "--bind_ip_all", "--replSet", "cma_memory_rca", "--wiredTigerCacheSizeGB", "0.25")
        mongo_created = True
        for _ in range(30):
            try:
                docker("exec", mongo, "mongosh", "--quiet", "--eval", "db.adminCommand({ping:1})", timeout=5)
                break
            except subprocess.SubprocessError:
                time.sleep(1)
        docker("exec", mongo, "mongosh", "--quiet", "--eval",
               "rs.initiate(" + json.dumps({"_id": "cma_memory_rca", "members": [{"_id": 0, "host": mongo + ":27017"}]}) + ")")
        wait_primary(mongo)
        for kind, binding in bindings:
            # An incomplete first variant must not skip the independent second variant.
            summary["variants"].append(run_variant(kind, binding, fixture, output / kind,
                                                   owner, mongo, private, key))
        summary["outcome"] = "completed" if all(v["outcome"] == "completed" for v in summary["variants"]) else "incomplete"
    finally:
        if mongo_created:
            remove_owned(mongo, owner)
        if network_created:
            label = docker("network", "inspect", "--format", '{{index .Labels "' + LABEL + '"}}', owner).strip()
            if label != owner:
                raise RuntimeError("network ownership changed")
            docker("network", "rm", owner)
        summary["ended_utc"] = utc()
        summary["cleanup"] = "owned containers and network removed"
        (output / "pair.json").write_text(json.dumps(summary, indent=2) + "\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("fixture", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt()))
    try:
        run(args.fixture, args.output)
    except BaseException as error:
        print(json.dumps({"outcome": "incomplete", "failure_type": type(error).__name__}))
        raise SystemExit(1)
