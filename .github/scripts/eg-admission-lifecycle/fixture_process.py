#!/usr/bin/env python3
"""Own one local admission fixture process; never issue workload/cluster requests."""
import argparse
import json
import math
import os
from pathlib import Path
import re
import select
import signal
import stat
import subprocess
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "scale-tests"))
from owned_process import OwnedGroup  # noqa: E402


class ProcessError(RuntimeError):
    """Fixed failure codes only; child output and private paths are not errors."""


def require(condition, code):
    if not condition:
        raise ProcessError(code)


def seconds(value, maximum):
    require(type(value) in (int, float) and math.isfinite(value) and 0 < value <= maximum,
            "invalid_process_budget")
    return value


class FixtureProcess:
    def __init__(self, binary, config, output, *, ready_seconds, lifetime_seconds, shutdown_seconds,
                 runtime_stack_bytes=None):
        self.binary, self.config, self.output = Path(binary), Path(config), Path(output)
        require(self.binary.is_absolute() and self.binary.is_file()
                and os.access(self.binary, os.X_OK), "fixture_binary_required")
        require(self.config.is_absolute(), "private_config_required")
        metadata = self.config.lstat()
        require(stat.S_ISREG(metadata.st_mode) and metadata.st_mode & 0o077 == 0
                and metadata.st_size <= 32768, "private_config_required")
        self.ready_seconds = seconds(ready_seconds, 120)
        self.lifetime_seconds = seconds(lifetime_seconds, 3600)
        self.shutdown_seconds = seconds(shutdown_seconds, 120)
        require(runtime_stack_bytes is None or (type(runtime_stack_bytes) is int
                and 1048576 <= runtime_stack_bytes <= 67108864), "invalid_runtime_stack_size")
        self.runtime_stack_bytes = runtime_stack_bytes
        require(self.ready_seconds <= self.lifetime_seconds, "ready_exceeds_lifetime")
        require(self.output.is_absolute(), "absolute_output_required")
        self.output.mkdir(mode=0o700, exist_ok=False)
        self.events = (self.output / "process-events.jsonl").open("x", encoding="utf8")
        os.chmod(self.output / "process-events.jsonl", 0o600)
        self.origin = time.monotonic()
        self.group = OwnedGroup()
        self.process = None
        self.buffer = b""
        self.discarding_line = False
        self.ready = self.stopped = self.error_observed = False
        self.stop_requested = False
        self.reason = None
        self.result = None
        self.record("registered", ready_seconds=self.ready_seconds, lifetime_seconds=self.lifetime_seconds,
                    shutdown_seconds=self.shutdown_seconds, runtime_stack_bytes=runtime_stack_bytes)

    def record(self, event, **fields):
        row = {"event": event, "elapsed_ms": (time.monotonic() - self.origin) * 1000, **fields}
        self.events.write(json.dumps(row, separators=(",", ":")) + "\n")
        self.events.flush()
        os.fsync(self.events.fileno())

    def start(self):
        require(self.process is None and self.result is None, "fixture_already_started")
        self.group.start()
        # The fixture gets its private config through the existing interface.
        # No bearer, database URI or arbitrary command is copied into evidence.
        environment = {"PATH": os.defpath, "EG_DIAGNOSTIC_CONFIG": str(self.config)}
        if self.runtime_stack_bytes is not None:
            environment["RUST_MIN_STACK"] = str(self.runtime_stack_bytes)
        self.process = self.group.spawn([str(self.binary)], env=environment,
            stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        os.set_blocking(self.process.stdout.fileno(), False)
        self.record("fixture_spawned")

    def drain(self):
        if self.process is None or self.process.stdout.closed:
            return
        # Limit each drain so a noisy child cannot starve the deadline checks.
        for _ in range(16):
            ready, _, _ = select.select([self.process.stdout], [], [], 0)
            if not ready:
                break
            try:
                chunk = os.read(self.process.stdout.fileno(), 4096)
            except BlockingIOError:
                break
            if not chunk:
                break
            self.buffer += chunk
            while b"\n" in self.buffer:
                line, self.buffer = self.buffer.split(b"\n", 1)
                if self.discarding_line:
                    self.discarding_line = False
                    continue
                line = line.rstrip(b"\r")
                if re.fullmatch(rb"eg_fixture status=READY database_sha256=[a-f0-9]{64}", line):
                    if not self.ready:
                        self.ready = True
                        self.record("fixture_ready")
                elif line == b"eg_fixture status=STOPPED ledger_retained=true database_retained=true":
                    self.stopped = True
                    self.record("fixture_stopped_marker")
                elif line.startswith(b"eg_fixture status=ERROR class="):
                    self.error_observed = True
                    self.record("fixture_error_marker")
            if len(self.buffer) > 4096:
                self.buffer = b""
                self.discarding_line = True

    def request_stop(self, *_):
        self.stop_requested = True

    def poll(self):
        self.drain()
        require(self.process is not None, "fixture_not_started")
        elapsed = time.monotonic() - self.origin
        if elapsed >= self.lifetime_seconds:
            self.reason = "lifetime_deadline"
        elif not self.ready and elapsed >= self.ready_seconds:
            self.reason = "ready_deadline"
        elif self.process.poll() is not None:
            self.reason = "unexpected_fixture_exit"
        elif self.error_observed:
            self.reason = "fixture_error"
        elif self.stop_requested:
            self.reason = "operator_stop"
        return self.reason

    def finish(self):
        if self.result is not None:
            return self.result
        started = time.monotonic()
        graceful = False
        observation_failed = False
        self.record("shutdown_start", reason=self.reason or "outer_failure")
        try:
            if self.process is not None:
                # Keep draining during TERM grace; a full pipe must not turn a
                # healthy fixture shutdown into an artificial timeout.
                self.group.signal(signal.SIGTERM)
                deadline = started + self.shutdown_seconds
                while True:
                    self.drain()
                    if not self.group.remaining(self.process):
                        graceful = time.monotonic() <= deadline
                        break
                    if time.monotonic() >= deadline:
                        break
                    time.sleep(min(.02, max(0, deadline - time.monotonic())))
        except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired):
            observation_failed = True
        cleanup = self.group.cleanup(self.process, grace=0, kill_wait=2)
        self.drain()
        exited = self.process is not None and self.process.poll() is not None
        code = self.process.returncode if exited else None
        if self.process is not None:
            self.process.stdout.close()
        cleanup_ok = cleanup["process_group_cleanup"] and not observation_failed
        lifecycle_ok = (self.reason == "operator_stop" and self.ready and self.stopped
                        and graceful and exited and code == 0 and cleanup_ok and not self.error_observed)
        self.result = {"status": "COMPLETE" if lifecycle_ok else "INCOMPLETE",
                       "reason": self.reason or "outer_failure", "ready_observed": self.ready,
                       "stopped_marker_observed": self.stopped, "error_marker_observed": self.error_observed,
                       "parent_exit_observed": exited, "exit_code": code,
                       "fixture_exit_within_shutdown_budget": graceful,
                       "shutdown_elapsed_ms": (time.monotonic() - started) * 1000,
                       "fixture_process_cleanup": cleanup_ok, **cleanup,
                       "gateway_process_finality": "NOT_OBSERVED", "application_acceptance": "NOT_EVALUATED",
                       "database_and_fixture_files_retained": True}
        self.record("shutdown_end", status=self.result["status"], parent_exit_observed=exited,
                    fixture_process_cleanup=cleanup_ok)
        with (self.output / "process-result.json").open("x", encoding="utf8") as stream:
            os.chmod(stream.name, 0o600)
            json.dump(self.result, stream, indent=2)
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        self.events.close()
        return self.result

    def run(self):
        previous = {value: signal.getsignal(value) for value in (signal.SIGINT, signal.SIGTERM)}
        for value in previous:
            signal.signal(value, self.request_stop)
        try:
            try:
                self.start()
                while not self.poll():
                    time.sleep(.02)
            except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired):
                self.reason = "outer_failure"
            return self.finish()
        finally:
            for value, handler in previous.items():
                signal.signal(value, handler)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("binary", "config", "out"):
        parser.add_argument("--" + name, required=True, type=Path)
    for name in ("ready-seconds", "lifetime-seconds", "shutdown-seconds"):
        parser.add_argument("--" + name, required=True, type=float)
    parser.add_argument("--runtime-stack-bytes", type=int)
    args = parser.parse_args()
    try:
        result = FixtureProcess(args.binary, args.config, args.out, ready_seconds=args.ready_seconds,
                                lifetime_seconds=args.lifetime_seconds,
                                shutdown_seconds=args.shutdown_seconds,
                                runtime_stack_bytes=args.runtime_stack_bytes).run()
        print(json.dumps(result))
        return 0 if result["status"] == "COMPLETE" else 2
    except (OSError, ValueError, RuntimeError):
        print(json.dumps({"status": "INCOMPLETE", "reason": "fixture_process_setup_failed"}))
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
