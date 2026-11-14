#!/usr/bin/env python3
"""Equal-count, loopback-only reads of an actual API process for incident #2056."""
import argparse
import concurrent.futures
import datetime
import http.client
import hashlib
import json
import math
import os
from pathlib import Path
import re
import subprocess
import time
from urllib.parse import urlsplit

METRICS = ("cma_runtime_alive_tasks", "cma_runtime_global_queue_depth",
           "cma_public_streams_active", "cma_http_in_flight", "cma_runtime_workers")
MAX_BODY = 8 * 1024 * 1024


class HttpResponseError(RuntimeError):
    def __init__(self, status, body_bytes):
        super().__init__("control response status/size mismatch")
        self.status = status
        self.body_bytes = body_bytes


def local_origin(value):
    parsed = urlsplit(value)
    if (parsed.scheme != "http" or parsed.hostname != "127.0.0.1" or not parsed.port
            or parsed.username or parsed.password or parsed.path not in ("", "/")
            or parsed.query or parsed.fragment):
        raise ValueError("an explicit http://127.0.0.1:PORT origin is required")
    return parsed


def private_json(path):
    path = Path(path)
    if path.is_symlink() or not path.is_file() or path.stat().st_mode & 0o077:
        raise ValueError("private regular config required")
    return json.loads(path.read_text())


def validate(config):
    if config.get("environment") != "isolated-local" or config.get("synthetic_identity") is not True:
        raise ValueError("isolated synthetic fixture required")
    for field in ("origin", "metrics_origin"):
        local_origin(config[field])
    if not re.fullmatch(r"cma-memory-rca-[a-z0-9-]{1,50}", config["container"]):
        raise ValueError("owned RCA container required")
    if not re.fullmatch(r"sha256:[0-9a-f]{64}", config["docker_image_id"]):
        raise ValueError("exact Docker image ID required")
    if config["source"] != "67b3fbc5df5f09fcecd9ddbbba723ae010df9cc6":
        raise ValueError("this comparison is scoped to exact source 67b3fbc5df5f")
    if config.get("docker_context", "desktop-linux") != "desktop-linux":
        raise ValueError("only the local desktop-linux Docker context is supported")
    for key, low, high, default in (("rounds", 20, 20, 20), ("round_seconds", 60, 60, 60),
                                   ("requests_per_round", 16, 16, 16),
                                   ("workers", 1, 4, 4), ("quiet_seconds", 12, 60, 12),
                                   ("idle_seconds", 12, 60, 60)):
        value = config.setdefault(key, default)
        if isinstance(value, bool) or not isinstance(value, int) or not low <= value <= high:
            raise ValueError(f"invalid {key}")
    if config["requests_per_round"] % config["workers"]:
        raise ValueError("request count must divide evenly across workers")
    return config


def connect(origin):
    parsed = local_origin(origin)
    return http.client.HTTPConnection(parsed.hostname, parsed.port, timeout=20)


def get(origin, path, expected=200):
    connection = connect(origin)
    try:
        connection.request("GET", path, headers={"Connection": "close"})
        response = connection.getresponse()
        body = response.read(MAX_BODY + 1)
        if response.status != expected or len(body) > MAX_BODY:
            raise HttpResponseError(response.status, len(body))
        return body
    finally:
        connection.close()


def read_events(origin, agent_id, expected_head, follow):
    connection = connect(origin)
    response = None
    received = 0
    events = 0
    event_type = ""
    data = []
    deadline = time.monotonic() + 30
    try:
        path = f"/api/v2/agents/{agent_id}/events?follow={'true' if follow else 'false'}"
        connection.request("GET", path, headers={"Connection": "close", "Accept": "text/event-stream"})
        response = connection.getresponse()
        if response.status != 200 or not response.getheader("Content-Type", "").startswith("text/event-stream"):
            raise RuntimeError("history stream refused")
        while True:
            if time.monotonic() >= deadline:
                raise RuntimeError("history request deadline exceeded")
            line = response.readline(MAX_BODY + 1)
            if not line:
                break
            received += len(line)
            if received > MAX_BODY:
                raise RuntimeError("history response exceeded fixture bound")
            line = line.rstrip(b"\r\n")
            if line.startswith(b"event:"):
                event_type = line[6:].strip().decode("utf8")
            elif line.startswith(b"data:"):
                data.append(line[5:].lstrip())
            elif not line:
                if event_type == "cma.agent.event":
                    event = json.loads(b"\n".join(data))
                    if event.get("agent_id") != agent_id or event.get("sequence") != events + 1:
                        raise RuntimeError("history fixture identity/order changed")
                    events += 1
                    if follow:
                        return {"requests": 1, "events": events, "bytes": received}
                elif event_type in ("cma.error", "error") or (data and b'"code"' in b"".join(data)):
                    raise RuntimeError("history stream terminated with an error")
                event_type, data = "", []
        if follow or events != expected_head:
            raise RuntimeError("history stream did not complete the fixed dataset")
        return {"requests": 1, "events": events, "bytes": received}
    finally:
        if response is not None:
            response.close()
        connection.close()


def parse_metrics(text):
    values = {name: 0.0 for name in METRICS}
    seen = set()
    for line in text.splitlines():
        match = re.fullmatch(r"([a-zA-Z_:][a-zA-Z0-9_:]*)(?:\{.*\})? ([^ ]+)(?: [0-9]+)?", line)
        if match and match[1] in values:
            value = float(match[2])
            if not math.isfinite(value):
                raise RuntimeError("nonfinite runtime metric")
            values[match[1]] += value
            seen.add(match[1])
    if seen != set(METRICS):
        raise RuntimeError("runtime/stream accounting metrics missing")
    return values


def process_fields(status, stat):
    fields = {}
    for line in status.splitlines():
        key, _, value = line.partition(":")
        if key in ("VmRSS", "RssAnon", "Threads"):
            fields[key] = int(value.split()[0])
        elif key == "Cpus_allowed_list":
            fields[key] = value.strip()
    # Field 22 is process start ticks; comm may contain spaces and parentheses.
    fields["start_ticks"] = int(stat[stat.rfind(")") + 2:].split()[19])
    if set(fields) != {"VmRSS", "RssAnon", "Threads", "start_ticks", "Cpus_allowed_list"}:
        raise RuntimeError("process RSS accounting missing")
    return fields


class Observer:
    def __init__(self, config):
        self.config = config
        self.identity = None
        self.started = time.monotonic()

    def docker(self, *arguments):
        return subprocess.check_output(["docker", "--context", "desktop-linux", *arguments],
                                       text=True, stderr=subprocess.PIPE, timeout=15)

    def sample(self, phase, round_number):
        template = ('{{.Id}} {{.Image}} {{.RestartCount}} {{.State.StartedAt}} {{.State.Running}} '
                    '{{.HostConfig.NanoCpus}} {{.HostConfig.Memory}} {{.HostConfig.CpusetCpus}}')
        identity = self.docker("inspect", "--format", template, self.config["container"]).strip().split()
        if (len(identity) != 8 or identity[1] != self.config["docker_image_id"]
                or identity[4] != "true" or identity[5:] != ["2000000000", "2147483648", "0-7"]):
            raise RuntimeError("actual API image/process identity mismatch")
        status = self.docker("exec", self.config["container"], "cat", "/proc/1/status")
        stat = self.docker("exec", self.config["container"], "cat", "/proc/1/stat")
        process = process_fields(status, stat)
        if process["Cpus_allowed_list"] != "0-7":
            raise RuntimeError("actual API CPU affinity differs from paired contract")
        identity.append(str(process["start_ticks"]))
        if self.identity is not None and identity != self.identity:
            raise RuntimeError("API process restarted or was replaced")
        self.identity = identity
        metrics = parse_metrics(get(self.config["metrics_origin"], "/metrics").decode("utf8"))
        if metrics["cma_runtime_workers"] != 8:
            raise RuntimeError("actual API Tokio worker count differs from paired contract")
        now = datetime.datetime.now(datetime.timezone.utc)
        # Python 3.9 accepts microseconds, while Docker emits nanoseconds.
        started_text = re.sub(r"(\.\d{6})\d+", r"\1", identity[3]).replace("Z", "+00:00")
        started = datetime.datetime.fromisoformat(started_text)
        return {"utc": now.isoformat(), "process_started_utc": identity[3],
                "process_age_seconds": (now - started).total_seconds(),
                "elapsed_seconds": time.monotonic() - self.started, "phase": phase, "round": round_number,
                "rss_kib": process["VmRSS"], "anonymous_kib": process["RssAnon"],
                "threads": process["Threads"], "cpu_affinity": process["Cpus_allowed_list"], **metrics}


def fit(rows):
    if len(rows) < 3:
        return None
    x = [row["elapsed_seconds"] / 60 for row in rows]
    y = [row["rss_kib"] / 1024 for row in rows]
    mx, my = sum(x) / len(x), sum(y) / len(y)
    xx = sum((v - mx) ** 2 for v in x)
    yy = sum((v - my) ** 2 for v in y)
    xy = sum((a - mx) * (b - my) for a, b in zip(x, y))
    if not xx:
        return None
    return {"samples": len(rows), "mib_per_minute": xy / xx,
            "first_utc": rows[0]["utc"], "last_utc": rows[-1]["utc"],
            "duration_seconds": (x[-1] - x[0]) * 60,
            "first_rss_mib": y[0], "last_rss_mib": y[-1], "peak_rss_mib": max(y),
            "r_squared": xy * xy / (xx * yy) if yy else None}


def run(config, output):
    config = validate(config)
    dataset = private_json(config["dataset"])
    if (dataset.get("environment") != "isolated-local" or dataset.get("synthetic_identity") is not True
            or dataset.get("head") != 515 or not re.fullmatch(r"agt_[a-zA-Z0-9_-]+", dataset["agent_id"])):
        raise ValueError("typed finite synthetic dataset required")
    output.mkdir(mode=0o700)
    observer = Observer(config)
    rows, counts = [], {"finite": 0, "follow_drop": 0, "control": 0, "events": 0, "bytes": 0,
                       "offered": 0, "failed": 0, "control_offered": 0, "control_failed": 0}
    result = {"source": config["source"], "docker_image_id": config["docker_image_id"],
              "outcome": "incomplete", "counts": counts,
              "dataset_sha256": hashlib.sha256(Path(config["dataset"]).read_bytes()).hexdigest(),
              "threshold_mib_per_minute": 1.0,
              "parameters": {key: config[key] for key in ("rounds", "round_seconds", "requests_per_round", "workers", "quiet_seconds", "idle_seconds")}}
    loop_started = None

    def sample(phase, index):
        row = observer.sample(phase, index)
        rows.append(row)
        with (output / "samples.jsonl").open("a") as stream:
            stream.write(json.dumps(row) + "\n")
        return row

    def quiet_until(phase, index, deadline):
        while time.monotonic() < deadline:
            time.sleep(max(0, min(5, deadline - time.monotonic())))
            sample(phase, index)

    try:
        sample("initial", 0)
        quiet_until("idle", 0, time.monotonic() + config["idle_seconds"])
        session = json.loads(get(config["origin"], "/auth/session"))
        if session.get("authenticated") is not True or session.get("subject") != dataset["subject"]:
            raise RuntimeError("actual API is not bound to the synthetic read owner")
        per_worker = config["requests_per_round"] // config["workers"]

        def work(follow):
            attempts = []
            for _ in range(per_worker):
                try:
                    row = read_events(config["origin"], dataset["agent_id"], dataset["head"], follow)
                    attempts.append({"outcome": "completed", **row})
                except Exception as error:
                    attempts.append({"outcome": "failed", "failure_type": type(error).__name__})
                    break
            return attempts

        loop_started = time.monotonic()
        result["loop_started_elapsed_seconds"] = loop_started - observer.started
        sample("loop_start", 1)
        for index in range(1, config["rounds"] + 1):
            round_deadline = loop_started + index * config["round_seconds"]
            sample("load_start", index)
            for path in ("/ready", "/openapi.json"):
                counts["control_offered"] += 1
                try:
                    get(config["origin"], path)
                    counts["control"] += 1
                except Exception:
                    counts["control_failed"] += 1
                    raise
            for follow, kind in ((False, "finite"), (True, "follow_drop")):
                with concurrent.futures.ThreadPoolExecutor(max_workers=config["workers"]) as pool:
                    futures = [pool.submit(work, follow) for _ in range(config["workers"])]
                    # Join every worker even on failure; no requests outlive a phase.
                    errors = []
                    for future in futures:
                        try:
                            attempts = future.result()
                            for attempt in attempts:
                                counts["offered"] += 1
                                with (output / "attempts.jsonl").open("a") as journal:
                                    journal.write(json.dumps({"round": index, "kind": kind, **attempt}) + "\n")
                                if attempt["outcome"] == "completed":
                                    counts[kind] += 1
                                    counts["events"] += attempt["events"]
                                    counts["bytes"] += attempt["bytes"]
                                else:
                                    counts["failed"] += 1
                                    errors.append(RuntimeError("history request failed"))
                        except Exception as error:
                            errors.append(error)
                    if errors:
                        raise RuntimeError("one or more fixed-count read workers failed") from errors[0]
            sample("load_end", index)
            if time.monotonic() + config["quiet_seconds"] > round_deadline:
                raise RuntimeError("fixed paced round cannot retain required quiescence")
            quiet_until("settling", index, time.monotonic() + config["quiet_seconds"])
            boundary = sample("quiescent", index)
            if boundary["cma_public_streams_active"] != 0 or boundary["cma_http_in_flight"] != 0:
                raise RuntimeError("HTTP/stream resources did not return to zero after disconnect")
            quiet_until("paced_idle", index, round_deadline)
        sample("loop_end", config["rounds"])
        result["loop_seconds"] = time.monotonic() - loop_started
        quiet_until("final_idle", config["rounds"], time.monotonic() + config["idle_seconds"])
        final = sample("final_quiescent", config["rounds"])
        if final["cma_public_streams_active"] or final["cma_http_in_flight"]:
            raise RuntimeError("final stream/request accounting did not quiesce")
        result["outcome"] = "completed"
    except BaseException as error:
        # Do not persist exception text: transports may include URLs or response content.
        result["failure_type"] = type(error).__name__
        raise
    finally:
        loop_rows = [row for row in rows if row["round"] > 0 and row["phase"] not in ("final_idle", "final_quiescent")]
        result["full_loop_fit"] = fit(loop_rows)
        result["all_observed_fit"] = fit(rows)
        result["after_first_loop_minute_fit"] = fit([row for row in loop_rows if row["elapsed_seconds"] >= result.get("loop_started_elapsed_seconds", 0) + 60])
        result["after_process_start_10_minutes_fit"] = fit([row for row in loop_rows if row["process_age_seconds"] >= 600])
        result["post_warmup_quiescent_fit"] = fit([row for row in rows if row["phase"] == "quiescent" and row["round"] > 2])
        result["final_idle_fit"] = fit([row for row in rows if row["phase"] == "final_idle"])
        result["observed_process_identity"] = observer.identity
        result["planned_reads"] = config["rounds"] * config["requests_per_round"] * 2
        result["not_offered_reads"] = result["planned_reads"] - counts["offered"]
        result["planned_controls"] = config["rounds"] * 2
        result["not_offered_controls"] = result["planned_controls"] - counts["control_offered"]
        result["quiescent_accounting"] = [{key: row[key] for key in ("utc", "round", "threads", *METRICS)} for row in rows if row["phase"] in ("quiescent", "final_quiescent")]
        complete = result["outcome"] == "completed" and result.get("loop_seconds", 0) >= 1200
        result["twenty_minute_rss_gate"] = ("PASS" if result["full_loop_fit"]["mib_per_minute"] < 1 else "FAIL") if complete else "NOT_EVALUATED"
        result["acceptance"] = "NOT_EVALUATED_requires_matched_variant_and_runtime_review"
        (output / "result.json").write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("config", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    try:
        run(private_json(args.config), args.output)
    except BaseException as error:
        print(json.dumps({"outcome": "incomplete", "failure_type": type(error).__name__}))
        raise SystemExit(1)
    print('{"outcome":"completed"}')
