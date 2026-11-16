#!/usr/bin/env python3
"""Bounded Kubernetes Events reader; only invokes kubectl get events.

Writes sanitized detections to stdout and private JSONL plus textfile metrics.
No exec, logs, Secrets, Kubernetes writes, notification calls, or repair actions.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
import fcntl
import json
import os
import re
from pathlib import Path
import subprocess
import tempfile
import time

from hook_events import Monitor, digest, target, utc

MAX_BYTES = 16 * 1024 * 1024


def read_events(kubectl, context, namespace):
    command = [kubectl, "--context", context, "--namespace", namespace,
               "get", "events", "--field-selector=involvedObject.kind=Pod",
               "--chunk-size=500", "--request-timeout=5s", "-o", "json"]
    # API errors can contain URLs or credentials; never emit their raw stderr.
    with tempfile.TemporaryFile() as out, tempfile.TemporaryFile() as error:
        try:
            result = subprocess.run(command, stdout=out, stderr=error, timeout=8, check=False)
        except subprocess.TimeoutExpired as exc:
            raise ValueError("events_read_timeout") from exc
        if result.returncode:
            raise ValueError("events_read_failed")
        if out.tell() > MAX_BYTES:
            raise ValueError("events_response_budget_exceeded")
        out.seek(0)
        return json.load(out)


def atomic(path, body):
    fd, temporary = tempfile.mkstemp(prefix=".hook-", dir=path.parent)
    try:
        with os.fdopen(fd, "w") as output:
            output.write(body)
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def parser():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--context", required=True)
    p.add_argument("--target", action="append", required=True, type=target,
                   help="service:namespace:pod-name-prefix; service is am, eg or web")
    p.add_argument("--output", required=True, type=Path)
    p.add_argument("--kubectl", default="kubectl")
    p.add_argument("--interval", type=float, default=5)
    p.add_argument("--duration", type=float, default=900)
    p.add_argument("--lookback", type=float, default=3600)
    p.add_argument("--once", action="store_true")
    p.add_argument("--replay", type=Path, help="Offline saved EventList; one namespace and one cycle")
    return p


def run(args, reader=read_events, clock=time.time, sleep=time.sleep, monotonic=time.monotonic):
    if not (1 <= args.interval <= 10 and 0 < args.duration <= 86400 and 0 <= args.lookback <= 86400):
        raise ValueError("watcher_time_budget_invalid")
    if not args.context or args.context.startswith("-") or len(args.context) > 512:
        raise ValueError("explicit_context_required")
    namespaces = sorted({t["namespace"] for t in args.target})
    if len(namespaces) > 3 or len(args.target) > 12 or len({tuple(t.values()) for t in args.target}) != len(args.target):
        raise ValueError("target_budget_or_duplicate")
    if args.replay and len(namespaces) != 1:
        raise ValueError("replay_requires_one_namespace")
    args.output.mkdir(parents=True, exist_ok=True, mode=0o700)
    identity = [args.context, args.target, "replay" if args.replay else "live"]
    if hasattr(args, "namespace_uid"):
        identity.append({"namespace_uid": args.namespace_uid})
    binding = digest(identity)
    started = clock()
    began = monotonic()
    state_path = args.output / "state.json"
    found = False
    with (args.output / "watcher.lock").open("a") as lock, (args.output / "alerts.jsonl").open("a") as journal:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        state = json.loads(state_path.read_text()) if state_path.exists() else None
        monitor = Monitor(args.target, started - args.lookback, binding, state)
        while True:
            # At most three independent namespace reads; one slow API call cannot
            # postpone the other services' snapshots through serial timeouts.
            pool = ThreadPoolExecutor(max_workers=3)
            reads = {} if args.replay else {ns: pool.submit(reader, args.kubectl, args.context, ns) for ns in namespaces}
            for namespace in namespaces:
                try:
                    if args.replay:
                        if args.replay.stat().st_size > MAX_BYTES:
                            raise ValueError("events_response_budget_exceeded")
                        payload = json.loads(args.replay.read_text())
                    else:
                        payload = reads[namespace].result()
                    alerts = monitor.consume(namespace, payload, clock())
                    found |= bool(alerts)
                except (ValueError, OSError) as error:
                    code = str(error) if re.fullmatch(r"(?:events|event|hook_event|watcher)_[a-z_]{1,70}", str(error)) else "events_read_invalid"
                    alerts = [monitor.failure(namespace, clock(), code)]
                for alert in alerts:
                    line = json.dumps(alert, sort_keys=True)
                    print(line, flush=True)
                    journal.write(line + "\n")
                journal.flush()
                os.fsync(journal.fileno())
            pool.shutdown(wait=True)
            now = clock()
            atomic(args.output / "metrics.prom", monitor.metrics(now))
            atomic(state_path, json.dumps(monitor.state, sort_keys=True) + "\n")
            status = {"schema": "cma-hook-watch-status-v1", "observed_at": utc(now),
                      "targets": args.target, "namespace_reads_ok": monitor.up,
                      "coverage_gap_observed": monitor.coverage_lost, "detected_in_run": found,
                      "source": "saved_events" if args.replay else "kubernetes_events",
                      "notification_delivery": "NOT_CONFIGURED", "mtta_seconds": None}
            atomic(args.output / "status.json", json.dumps(status, sort_keys=True) + "\n")
            if args.once or args.replay or monotonic() - began >= args.duration:
                return 2 if monitor.coverage_lost else 1 if found else 0
            sleep(min(args.interval, max(0, args.duration - (monotonic() - began))))


def main():
    os.umask(0o077)
    try:
        return run(parser().parse_args())
    except KeyboardInterrupt:
        return 130
    except (ValueError, OSError):
        print(json.dumps({"schema": "cma-hook-watch-gap-v1", "severity": "critical",
                          "code": "watcher_failed", "delivery": "NOT_SENT"}), flush=True)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
