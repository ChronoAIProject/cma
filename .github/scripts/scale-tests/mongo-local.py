#!/usr/bin/env python3
"""Run a compiled EG process test with an owned, authenticated Mongo replica set.

Reuse the repository's pinned Mongo image and readiness initializer. No existing
cluster/database is accepted. Credentials stay in process environments and are
removed with the disposable container. Only sanitized results leave the runner.
"""

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import secrets
import re
import signal
import subprocess
import tempfile
import time


IMAGE = "mongo:8.0.13@sha256:cf340b1e5283843c63eb12999922f20c463ae31285f746d30f05dcc21cd1d47c"


def command(arguments, environment=None, timeout=30):
    result = subprocess.run(arguments, env=environment, capture_output=True, timeout=timeout)
    if result.returncode:
        raise RuntimeError("local_fixture_command_failed")
    return result.stdout.decode().strip()


def cleanup_container(container, label, suffix):
    probe = subprocess.run(["docker", "inspect", "--format",
                            f'{{{{index .Config.Labels "{label}"}}}}', container],
                           capture_output=True, timeout=15)
    if probe.returncode:
        if b"No such object:" in probe.stderr or b"No such container:" in probe.stderr:
            return
        raise RuntimeError("container_cleanup_unconfirmed")
    if probe.stdout.decode().strip() != suffix:
        raise RuntimeError("cleanup_ownership_mismatch")
    command(["docker", "rm", "--force", "--volumes", container])


def run(binary, directory, test_name):
    directory.mkdir(parents=True, exist_ok=True, mode=0o700)
    target = directory / "mongo-process-results.json"
    if target.exists():
        raise RuntimeError("existing_evidence_refused")
    suffix = secrets.token_hex(12)
    container = f"cma-scale-test-{suffix}"
    label = "dev.chrono.cma.scale-test"
    environment = dict(os.environ)
    environment.update(MONGO_INITDB_ROOT_USERNAME="contract-admin",
                       MONGO_INITDB_ROOT_PASSWORD=secrets.token_hex(24),
                       CMA_IM_REPLICA_KEY=secrets.token_hex(64))
    started = time.monotonic()
    digest = hashlib.sha256()
    with binary.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    result = {"environment": "local_disposable_mongo", "status": "BLOCKED", "cleanup": False,
              "binary_sha256": digest.hexdigest(), "test_name": test_name,
              "mongo_image": IMAGE, "started_at": datetime.now(timezone.utc).isoformat(),
              "mongo_limits": {"cpus": 1, "memory_mib": 1024, "db_tmpfs_mib": 1024, "oplog_mib": 64}}
    attempted = False
    try:
        # A failed/timed-out docker run may still have created the named container.
        attempted = True
        command(["docker", "run", "--detach", "--name", container, "--label", f"{label}={suffix}",
                 "--publish", "127.0.0.1::27017", "--memory", "1g", "--memory-swap", "1g", "--cpus", "1",
                 "--pids-limit", "256", "--tmpfs", "/data/db:rw,size=1073741824",
                 "--tmpfs", "/data/configdb:rw,size=16777216",
                 "--env", "MONGO_INITDB_ROOT_USERNAME", "--env", "MONGO_INITDB_ROOT_PASSWORD",
                 "--env", "CMA_IM_REPLICA_KEY", "--entrypoint", "sh", IMAGE, "-c",
                 'umask 077; printf "%s" "$CMA_IM_REPLICA_KEY" > /tmp/replica-key; '
                 'chown mongodb:mongodb /tmp/replica-key; exec docker-entrypoint.sh mongod '
                 '--replSet cma-im-contract --keyFile /tmp/replica-key '
                 '--wiredTigerCacheSizeGB 0.25 --oplogSize 64'],
                environment, timeout=60)
        initializer = Path(__file__).resolve().parent.parent / "authenticated-mongo-readiness.js"
        script = initializer.read_text()
        for _ in range(60):
            probe = subprocess.run(["docker", "exec", container, "mongosh", "--quiet",
                                    "mongodb://127.0.0.1:27017/admin?serverSelectionTimeoutMS=1000&connectTimeoutMS=1000",
                                    "--eval", script], capture_output=True, timeout=10)
            if probe.returncode == 0:
                break
            time.sleep(1)
        else:
            raise RuntimeError("mongo_readiness_deadline")
        endpoint = command(["docker", "port", container, "27017/tcp"])
        host, port = endpoint.split(":")
        if host != "127.0.0.1" or not port.isdigit():
            raise RuntimeError("nonlocal_mongo_refused")
        password = environment["MONGO_INITDB_ROOT_PASSWORD"]
        environment["CMA_IM_TEST_MONGODB_URI"] = f"mongodb://contract-admin:{password}@{endpoint}/admin?directConnection=true"
        with tempfile.TemporaryDirectory(prefix="process-fixture-", dir=directory) as temporary:
            environment["TMPDIR"] = temporary
            process = subprocess.Popen([str(binary), test_name, "--ignored", "--test-threads=1"],
                                       env=environment, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                       start_new_session=True)
            try:
                output, _ = process.communicate(timeout=120)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGTERM)
                try:
                    output, _ = process.communicate(timeout=10)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    output, _ = process.communicate(timeout=10)
                raise RuntimeError("process_test_deadline") from None
            passed = re.search(rb"test result: ok\. 1 passed; 0 failed;", output) is not None
            result.update(status="PASS" if process.returncode == 0 and passed else "FAIL",
                          exit_code=process.returncode, output_sha256=hashlib.sha256(output).hexdigest(),
                          tests_expected=1, tests_passed=1 if passed else 0)
            result["numeric_assertions"] = [int(value) for value in re.findall(rb"(?:left|right):\s+(\d+)", output)]
            result["completed_replica_counts"] = [int(value) for value in re.findall(
                rb"replica_fixture replicas=([123]) status=PASS", output)]
            result["ingress_rejections"] = [value.decode() for value in re.findall(
                rb"ingress_rejection=(agent_command_key_conflict|event_id_content_conflict|provider_busy|conflict|unclassified)", output)]
            result["migration_diagnostics"] = [{"stage": stage.decode(), "code": int(code), "kind": kind.decode()}
                for stage, code, kind in re.findall(rb"fixture_diagnostic stage=(create|index|validator|marker|other) code=(-?\d+) kind=(field_name|disk_space|index_name|authorization|unclassified|non_command)", output)]
            result["failure_codes"] = [code for text, code in [
                (b"same admitted event must remain accepted", "cross_ingress_not_accepted"),
                (b"one provider command identity survives", "provider_admission_not_unique"),
                (b"gateway fixture migration failed", "fixture_migration_failed"),
                (b"Elapsed(())", "gateway_start_or_shutdown_deadline")
            ] if text in output]
    except (RuntimeError, subprocess.TimeoutExpired, OSError, ValueError):
        result.update(status="BLOCKED", code="local_fixture_execution_incomplete")
    finally:
        try:
            if attempted:
                cleanup_container(container, label, suffix)
            result["cleanup"] = True
        except (RuntimeError, subprocess.TimeoutExpired, OSError):
            result.update(status="BLOCKED", cleanup=False, code="container_cleanup_unconfirmed")
        result["elapsed_seconds"] = time.monotonic() - started
        result["finished_at"] = datetime.now(timezone.utc).isoformat()
        target.write_text(json.dumps(result, indent=2) + "\n")
        target.chmod(0o600)
    print(json.dumps(result))
    return 0 if result["status"] == "PASS" else 2


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--test-name", default="runner_binary::replicas::")
    arguments = parser.parse_args()
    if not arguments.binary.is_file():
        raise SystemExit("BLOCKED: compiled test binary required")
    if not re.fullmatch(r"runner_binary::[a-z_0-9:]+", arguments.test_name):
        raise SystemExit("BLOCKED: explicit runner test name required")
    raise SystemExit(run(arguments.binary.resolve(), arguments.out.resolve(), arguments.test_name))
