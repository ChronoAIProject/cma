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
import shutil
import subprocess
import tempfile
import time

from owned_process import OwnedGroup
from replica_observations import phase_observations
from host_capture import collect_directory


IMAGE = "mongo:8.0.13@sha256:cf340b1e5283843c63eb12999922f20c463ae31285f746d30f05dcc21cd1d47c"


class FixtureError(RuntimeError):
    """A fixed runner code; never derive this value from external exception text."""
    def __init__(self, code):
        super().__init__(code)
        self.code = code


def observations(output):
    result = {"output_sha256": hashlib.sha256(output).hexdigest(), "tests_expected": 1,
              "tests_passed": int(re.search(rb"test result: ok\. 1 passed; 0 failed;", output) is not None)}
    result["numeric_assertions"] = [int(value) for value in re.findall(rb"(?:left|right):\s+(\d+)", output)]
    result["completed_replica_counts"] = [int(value) for value in re.findall(
        rb"replica_fixture replicas=([123]) status=PASS", output)]
    result["completed_ownership_cohorts"] = [{"replicas": int(count), "enabled": enabled == b"true"}
        for count, enabled in re.findall(
            rb"replica_fixture replicas=([123]) status=PASS ownership=(true|false)", output)]
    result["ingress_rejections"] = [value.decode() for value in re.findall(
        rb"ingress_rejection=(agent_command_key_conflict|event_id_content_conflict|provider_busy|conflict|routed_admission_unavailable|routed_authority_changed_retry|routed_provider_replay_unavailable|custom_durable_admission_unavailable|provider_unavailable|internal|unclassified)", output)]
    result["failed_cohort_observations"] = [
        {"replicas": int(count), "enabled": enabled == b"true", "accepted_responses": int(accepted),
         "durable_plan_after_responses": plan == b"true", "targets_present_after_responses": int(targets)}
        for count, enabled, accepted, plan, targets in re.findall(
            rb"replica_failure replicas=([123]) ownership=(true|false) accepted=(\d+) durable_plan=(true|false) target_present=(\d+)", output)]
    result["migration_diagnostics"] = [{"stage": stage.decode(), "code": int(code), "kind": kind.decode()}
        for stage, code, kind in re.findall(rb"fixture_diagnostic stage=(create|index|validator|marker|other) code=(-?\d+) kind=(field_name|disk_space|index_name|authorization|unclassified|non_command)", output)]
    result["failure_codes"] = [code for text, code in [
        (b"same admitted event must remain accepted", "cross_ingress_not_accepted"),
        (b"one provider command identity survives", "provider_admission_not_unique"),
        (b"gateway fixture migration failed", "fixture_migration_failed"),
        (b"Elapsed(())", "gateway_start_or_shutdown_deadline")
    ] if text in output]
    result.update(phase_observations(output))
    return result


def command(arguments, environment=None, timeout=30):
    result = subprocess.run(arguments, env=environment, capture_output=True, timeout=timeout)
    if result.returncode:
        raise FixtureError("local_fixture_command_failed")
    return result.stdout.decode().strip()


def cleanup_container(container, label, suffix):
    probe = subprocess.run(["docker", "inspect", "--format",
                            f'{{{{index .Config.Labels "{label}"}}}}', container],
                           capture_output=True, timeout=15)
    if probe.returncode:
        if b"No such object:" in probe.stderr or b"No such container:" in probe.stderr:
            return
        raise FixtureError("container_cleanup_unconfirmed")
    if probe.stdout.decode().strip() != suffix:
        raise FixtureError("cleanup_ownership_mismatch")
    command(["docker", "rm", "--force", "--volumes", container])


def run(binary, application, directory, test_name):
    directory.mkdir(parents=True, exist_ok=True, mode=0o700)
    target = directory / "mongo-process-results.json"
    if target.exists():
        raise FixtureError("existing_evidence_refused")
    suffix = secrets.token_hex(12)
    container = f"cma-scale-test-{suffix}"
    label = "dev.chrono.cma.scale-test"
    application_sha256 = hashlib.sha256(application.read_bytes()).hexdigest()
    evidence = directory / "evidence"
    evidence.mkdir(mode=0o700)
    environment = dict(os.environ)
    environment["CMAEG_HOST_EVIDENCE"] = str(evidence)
    environment["CMAEG_HOST_APPLICATION_SHA256"] = application_sha256
    environment["CMAEG_HOST_APPLICATION"] = str(application)
    environment.update(MONGO_INITDB_ROOT_USERNAME="contract-admin",
                       MONGO_INITDB_ROOT_PASSWORD=secrets.token_hex(24),
                       CMA_IM_REPLICA_KEY=secrets.token_hex(64))
    started = time.monotonic()
    digest = hashlib.sha256()
    with binary.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    result = {"environment": "local_disposable_mongo", "status": "BLOCKED", "cleanup": False,
              "binary_sha256": digest.hexdigest(), "application_sha256": application_sha256, "test_name": test_name,
              "mongo_image": IMAGE, "started_at": datetime.now(timezone.utc).isoformat(),
              "mongo_limits": {"cpus": 1, "memory_mib": 1024, "db_tmpfs_mib": 1024, "oplog_mib": 64}}
    attempted = False
    phase = "container_create"
    process = None
    group = None
    temporary = None
    output = None
    result.update(timed_out=False, censored=True, failure_phase=None, error_class=None,
                  exit_code=None, termination_signal=None, output_sha256=None)
    try:
        # A pinned digest makes a repeated pull safe; one registry hiccup must
        # not fail the fixture before any container exists.
        phase = "image_pull"
        command(["bash", str(Path(__file__).resolve().parent.parent / "pull-pinned-image.sh"), IMAGE],
                environment, timeout=700)
        phase = "container_create"
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
        phase = "mongo_initialize"
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
            raise FixtureError("mongo_readiness_deadline")
        phase = "endpoint_discovery"
        endpoint = command(["docker", "port", container, "27017/tcp"])
        host, port = endpoint.split(":")
        if host != "127.0.0.1" or not port.isdigit():
            raise FixtureError("nonlocal_mongo_refused")
        password = environment["MONGO_INITDB_ROOT_PASSWORD"]
        environment["CMA_IM_TEST_MONGODB_URI"] = f"mongodb://contract-admin:{password}@{endpoint}/admin?directConnection=true"
        phase = "test_spawn"
        temporary = tempfile.mkdtemp(prefix="process-fixture-", dir=directory)
        environment["TMPDIR"] = temporary
        group = OwnedGroup()
        group.start()
        process = group.spawn([str(binary), test_name, "--ignored", "--nocapture", "--test-threads=1"],
                              env=environment, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        phase = "test_wait"
        try:
            output, _ = process.communicate(timeout=120)
        except subprocess.TimeoutExpired as error:
            result.update(timed_out=True, code="process_test_deadline")
            output = error.output or b""
            raise FixtureError("process_test_deadline") from None
        result.update(status="PASS" if process.returncode == 0 and observations(output)["tests_passed"] == 1 else "FAIL",
                      censored=False)
        if result["status"] == "FAIL":
            result.update(failure_phase="test_result", error_class="test_failure")
    except (RuntimeError, subprocess.TimeoutExpired, OSError, ValueError) as error:
        error_class = ("command_timeout" if isinstance(error, subprocess.TimeoutExpired) else
                       "os_error" if isinstance(error, OSError) else
                       "value_error" if isinstance(error, ValueError) else "fixture_error")
        result.update(status="BLOCKED", failure_phase=phase, error_class=error_class,
                      code=error.code if isinstance(error, FixtureError) else
                      result.get("code", "local_fixture_execution_incomplete"))
        if isinstance(error, subprocess.TimeoutExpired) or (isinstance(error, FixtureError) and
                error.code in {"process_test_deadline", "mongo_readiness_deadline"}):
            result["timed_out"] = True
    finally:
        # Every path (normal exit, failure, timeout, spawn/pipe error) observes
        # the entire reserved group before disposing anything it can still use.
        custody = group.cleanup(process) if group is not None else {"process_group_cleanup": True}
        result.update(custody)
        if process is not None:
            try:
                final_output, _ = process.communicate(timeout=2)
                output = final_output or output
            except subprocess.TimeoutExpired as error:
                output = error.output or output
                result["censored"] = True
            except (OSError, ValueError):
                result["censored"] = True
            result["exit_code"] = process.returncode
            result["termination_signal"] = -process.returncode if process.returncode is not None and process.returncode < 0 else None
        if output is not None:
            result.update(observations(output))
        result["primary_status"] = result["status"]
        result["test_parent_cleanup"] = process is None or process.returncode is not None
        result["process_cleanup"] = result["test_parent_cleanup"] and result["process_group_cleanup"]
        result["fixture_directory_retained"] = temporary is not None
        result["container_cleanup"] = not attempted
        if not result["process_cleanup"]:
            result.update(status="BLOCKED", cleanup_failure_phase="process_termination", censored=True)
            result.setdefault("code", "process_cleanup_unconfirmed")
        else:
            try:
                capture_ok = True
                if temporary is not None:
                    try:
                        capture = collect_directory(Path(temporary), evidence, application_sha256)
                    except (OSError, ValueError, TypeError):
                        capture = {"complete": False, "failures": ["host_capture_failed"]}
                    result["host_capture"] = capture
                    capture_ok = capture["complete"]
                    retain_failed = result["primary_status"] != "PASS"
                    result["private_failed_trace_retained"] = retain_failed
                    if capture_ok and not retain_failed:
                        shutil.rmtree(temporary)
                        result["fixture_directory_retained"] = False
                    elif not capture_ok:
                        result.update(status="BLOCKED", code="host_capture_incomplete",
                                      cleanup_failure_phase="evidence_collection")
                if attempted:
                    cleanup_container(container, label, suffix)
                result["container_cleanup"] = True
                result["cleanup"] = capture_ok and not result["fixture_directory_retained"]
            except (RuntimeError, subprocess.TimeoutExpired, OSError) as error:
                result.update(status="BLOCKED", cleanup=False, primary_code=result.get("code"),
                              code=error.code if isinstance(error, FixtureError) else "fixture_cleanup_unconfirmed", cleanup_failure_phase="fixture_cleanup",
                              cleanup_timed_out=isinstance(error, subprocess.TimeoutExpired))
        try:
            unchanged = hashlib.sha256(application.read_bytes()).hexdigest() == application_sha256
        except OSError:
            unchanged = False
        if not unchanged:
            result.update(status="BLOCKED", code="application_changed_during_run")
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
    parser.add_argument("--application", required=True, type=Path)
    parser.add_argument("--test-name", default="runner_binary::replicas::independent_replicas_share_storage_and_preserve_one_provider_admission")
    arguments = parser.parse_args()
    if not arguments.binary.is_file() or not arguments.application.is_file():
        raise SystemExit("BLOCKED: compiled test binary required")
    if not re.fullmatch(r"runner_binary::[a-z_0-9:]+", arguments.test_name):
        raise SystemExit("BLOCKED: explicit runner test name required")
    raise SystemExit(run(arguments.binary.resolve(), arguments.application.resolve(), arguments.out.resolve(), arguments.test_name))
