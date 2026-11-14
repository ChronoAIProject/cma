#!/usr/bin/env python3
"""Lead 23:55 parallel empty-DB API qualification; fixed 1 GET/s and 30s RSS samples."""
import argparse
import base64
import concurrent.futures
import json
from pathlib import Path
import secrets
import signal
import subprocess
import threading
import time
import traceback

import pair
import repeat

ROUTES = ("/livez", "/ready", "/openapi.json")
SECONDS = 1200
SAMPLE_SECONDS = 30


def workload(config, start, output, stop):
    result = {"planned": SECONDS, "offered": 0, "completed": 0, "failed": 0,
              "bytes": 0, "late_over_one_second": 0,
              "routes": {route: {"offered": 0, "completed": 0, "failed": 0} for route in ROUTES}}
    with (output / "requests.jsonl").open("x") as journal:
        for index in range(SECONDS):
            if stop.wait(max(0, start + index - time.monotonic())):
                break
            route = ROUTES[index % len(ROUTES)]
            attempt = {"utc": pair.utc(), "index": index, "route": route,
                       "lag_seconds": max(0, time.monotonic() - (start + index))}
            result["offered"] += 1
            result["routes"][route]["offered"] += 1
            result["late_over_one_second"] += attempt["lag_seconds"] > 1
            request_start = time.monotonic()
            try:
                body = repeat.get(config["origin"], route)
                result["completed"] += 1
                result["routes"][route]["completed"] += 1
                result["bytes"] += len(body)
                attempt.update(outcome="completed", bytes=len(body))
                del body
            except Exception as error:
                result["failed"] += 1
                result["routes"][route]["failed"] += 1
                attempt.update(outcome="failed", failure_type=type(error).__name__)
            attempt["duration_seconds"] = time.monotonic() - request_start
            journal.write(json.dumps(attempt) + "\n")
            journal.flush()
    result["not_offered"] = SECONDS - result["offered"]
    result["ended_utc"] = pair.utc()
    return result


def describe(rows, field):
    values = [row[field] for row in rows]
    return {"first": values[0], "peak": max(values), "last": values[-1], "delta": values[-1] - values[0]}


def measure(configs, output):
    observers = {kind: repeat.Observer(config) for kind, config in configs.items()}
    rows = {kind: [] for kind in configs}
    observation_errors = {kind: [] for kind in configs}
    stop = threading.Event()
    result = {"routes": list(ROUTES), "planned_seconds": SECONDS, "requests_per_second_each": 1,
              "sample_interval_seconds": SAMPLE_SECONDS, "variants": {}}

    def sample(kind, phase, index):
        row = observers[kind].sample(phase, index)
        rows[kind].append(row)
        with (output / kind / "samples.jsonl").open("a") as journal:
            journal.write(json.dumps(row) + "\n")
        return row

    for kind in configs:
        sample(kind, "initial", 0)
    start = time.monotonic()
    result["loop_started_utc"] = pair.utc()
    (output / "loop-start.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"event": "loop_started", "utc": result["loop_started_utc"], "seconds": SECONDS}), flush=True)
    try:
        with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
            futures = {kind: pool.submit(workload, config, start, output / kind, stop)
                       for kind, config in configs.items()}
            try:
                for index in range(1, SECONDS // SAMPLE_SECONDS + 1):
                    time.sleep(max(0, start + index * SAMPLE_SECONDS - time.monotonic()))
                    for kind in configs:
                        try:
                            sample(kind, "loop", index)
                        except Exception as error:
                            observation_errors[kind].append({"utc": pair.utc(), "index": index, "failure_type": type(error).__name__})
                result["loop_ended_utc"] = pair.utc()
                result["actual_loop_seconds"] = time.monotonic() - start
                for kind, future in futures.items():
                    result["variants"][kind] = {"counts": future.result()}
            finally:
                stop.set()
        time.sleep(12)
        for kind in configs:
            try:
                sample(kind, "quiescent", 41)
            except Exception as error:
                observation_errors[kind].append({"utc": pair.utc(), "index": 41, "failure_type": type(error).__name__})
    finally:
        for kind in configs:
            observed = rows[kind]
            variant = result["variants"].setdefault(kind, {})
            variant["observation_errors"] = observation_errors[kind]
            loop = [row for row in observed if row["phase"] != "quiescent"]
            variant.update(full_loop_fit=repeat.fit(loop),
                           after_first_loop_minute_fit=repeat.fit(loop[2:]),
                           after_process_age_10min_fit=repeat.fit([row for row in loop if row["process_age_seconds"] >= 600]),
                           process_identity=observers[kind].identity,
                           image=configs[kind]["docker_image_id"])
            if observed:
                variant["rss_kib"] = describe(loop, "rss_kib")
                variant["anonymous_kib"] = describe(loop, "anonymous_kib")
                variant["task_range"] = [min(row["cma_runtime_alive_tasks"] for row in loop), max(row["cma_runtime_alive_tasks"] for row in loop)]
                variant["final_observation"] = observed[-1]
            counts = variant.get("counts", {})
            variant["complete"] = (counts.get("completed") == SECONDS and not counts.get("failed")
                                   and len(loop) == 41 and result.get("actual_loop_seconds", 0) >= SECONDS
                                   and observed[-1]["phase"] == "quiescent")
        # The old image's positive-slope interpretation remains explicit, with full slope/R²/delta evidence.
        musl = result["variants"].get("musl", {})
        zig = result["variants"].get("zig", {})
        result["corrected_under_1_gate"] = (
            "PASS" if musl["full_loop_fit"]["mib_per_minute"] < 1 else "FAIL"
        ) if musl.get("complete") else "INCOMPLETE"
        result["paired_verdict"] = "REQUIRES_OLD_CLEAR_POSITIVE_SLOPE_REVIEW" if all(v.get("complete") for v in (musl, zig)) else "INCOMPLETE"
        (output / "measurement.json").write_text(json.dumps(result, indent=2) + "\n")
    return result


def run(output):
    output = output.resolve()
    output.mkdir(mode=0o700)
    private = output / "private"
    private.mkdir(mode=0o700)
    pair.private_write(private / "provider-key", secrets.token_urlsafe(32))
    (private / "provider-key").chmod(0o444)
    key = base64.b64encode(secrets.token_bytes(32)).decode()
    owner = "cma-memory-rca-" + secrets.token_hex(6)
    mongo = owner + "-mongo"
    owned = []
    network = False
    result = {"started_utc": pair.utc(), "source": pair.SOURCE, "owner": owner,
              "grant": "direct LEAD 23:55 parallel empty-DB API qualification",
              "routes": list(ROUTES), "outcome": "incomplete", "images": {}}
    try:
        bindings = {"musl": pair.image(pair.NEW), "zig": pair.image(pair.OLD)}
        result["images"] = bindings
        pair.docker("network", "create", "--label", f"{pair.LABEL}={owner}", owner)
        network = True
        pair.docker("run", "--detach", "--pull", "never", "--name", mongo, "--hostname", mongo,
                    "--label", f"{pair.LABEL}={owner}", "--network", owner, "--cpus", "0.5",
                    "--memory", "768m", "--memory-swap", "768m", "--pids-limit", "256",
                    "--publish", "127.0.0.1:18917:27017", pair.MONGO,
                    "--bind_ip_all", "--replSet", "cma_memory_rca", "--wiredTigerCacheSizeGB", "0.25")
        owned.append(mongo)
        for _ in range(30):
            try:
                pair.docker("exec", mongo, "mongosh", "--quiet", "--eval", "db.adminCommand({ping:1})", timeout=5)
                break
            except subprocess.SubprocessError:
                time.sleep(1)
        pair.docker("exec", mongo, "mongosh", "--quiet", "--eval",
                    "rs.initiate(" + json.dumps({"_id": "cma_memory_rca", "members": [{"_id": 0, "host": mongo + ":27017"}]}) + ")")
        pair.wait_primary(mongo)
        empty_path = "/apis/sandbox.opensandbox.io/v1alpha1/namespaces/memory-rca/batchsandboxes"
        responder = "const http=require('http');const URL=require('url').URL; http.createServer((q,r)=>{const path=new URL(q.url,'http://local').pathname;const allowed=['batchsandboxes','sandboxes','sandboxclaims','sandboxwarmpools','sandboxtemplates','persistentvolumeclaims','pods'];const ok=q.method==='GET' && path.includes('/namespaces/memory-rca/') && allowed.includes(path.split('/').pop());r.writeHead(ok?200:404,{'Content-Type':'application/json'});r.end(JSON.stringify(ok?{apiVersion:'sandbox.opensandbox.io/v1alpha1',kind:'BatchSandboxList',metadata:{resourceVersion:'1'},items:[]}:{apiVersion:'v1',kind:'Status',status:'Failure',reason:'NotFound',code:404}));}).listen(18081,'0.0.0.0'); await new Promise(()=>{});"
        pair.docker("exec", "-d", mongo, "mongosh", "--nodb", "--quiet", "--eval", "(async()=>{" + responder + "})().catch(e=>{print(e.name);quit(2)});")
        pair.docker("exec", mongo, "mongosh", "--nodb", "--quiet", "--eval",
                    "(async()=>{const http=require('http');for(let i=0;i<20;i++){try{const code=await new Promise((resolve,reject)=>http.get('http://127.0.0.1:18081" + empty_path + "?limit=1',r=>{r.resume();resolve(r.statusCode);}).on('error',reject));if(code===200){print('ready');quit(0);}}catch{} await new Promise(r=>setTimeout(r,250));}quit(2);})().catch(e=>{print(e.name);quit(2)});")
        kubeconfig = {"apiVersion": "v1", "kind": "Config", "current-context": "synthetic",
                      "clusters": [{"name": "synthetic", "cluster": {"server": f"http://{mongo}:18081"}}],
                      "contexts": [{"name": "synthetic", "context": {"cluster": "synthetic", "user": "synthetic", "namespace": "memory-rca"}}],
                      "users": [{"name": "synthetic", "user": {"token": "synthetic-local-only"}}]}
        pair.private_write(private / "kubeconfig", json.dumps(kubeconfig))
        (private / "kubeconfig").chmod(0o444)
        result["startup_dependency"] = "GET-only synthetic empty namespaced runtime/PVC lists; no runtime resources, all mutations refused; native pool targets zero"
        configs = {}
        database_password = secrets.token_hex(24)
        for offset, (kind, binding) in enumerate(bindings.items()):
            (output / kind).mkdir()
            container = owner + "-" + kind
            database = "cma_memory_rca_" + owner.removeprefix("cma-memory-rca-") + "_" + kind
            settings = pair.environment("synthetic-memory-reader", mongo, database, key)
            role = json.loads(pair.docker("run", "--rm", "--pull", "never", "--network", "none",
                            "--platform", "linux/amd64", "--cpus", "0.5", "--memory", "256m",
                            "--label", f"{pair.LABEL}={owner}", "--env", f"CHRONO_MONGODB_DATABASE={database}",
                            binding["id"], "--print-runtime-role"))
            # Create only the API's exact generated role and a synthetic principal in this fresh DB.
            setup = "const input=JSON.parse(require('fs').readFileSync(0,'utf8')); const target=db.getSiblingDB(input.database); if(!target.runCommand(input.role).ok) throw Error('role setup'); target.createUser({user:'api-memory',pwd:input.password,roles:[{role:'cma_am_runtime',db:input.database}]}); print('ready');"
            pair.docker("exec", "-i", mongo, "mongosh", "--quiet", "--eval", setup,
                        input=json.dumps({"database": database, "password": database_password, "role": role}))
            settings["CHRONO_MONGODB_URI"] = f"mongodb://api-memory:{database_password}@{mongo}:27017/?replicaSet=cma_memory_rca&authSource={database}"
            settings["KUBECONFIG"] = "/run/synthetic-kubeconfig"
            settings.update({"CHRONO_SANDBOX_PROVIDER": "agent-sandbox", "CHRONO_RETAIN_OPENSANDBOX": "false",
                             "CHRONO_AGENT_SANDBOX_ENABLED": "true",
                             "CHRONO_AGENT_SANDBOX_CONTROL_KEY_FILE": "/run/synthetic-provider-key",
                             "CHRONO_AGENT_SANDBOX_HOME_MODE": "native_vct_full_home_v2",
                             "CHRONO_AGENT_SANDBOX_HOME_STORAGE_CLASS": "synthetic-local",
                             "CHRONO_AGENT_SANDBOX_HOME_CAPACITY_GIB": "1"})
            settings["CHRONO_SERVER_PUBLIC_URL"] = "https://api-memory.invalid"
            settings["CHRONO_SERVER_CORS_ORIGIN"] = "https://api-memory.invalid"
            envfile = private / (kind + ".env")
            pair.private_write(envfile, "".join(f"{name}={value}\n" for name, value in settings.items()))
            config = {"source": pair.SOURCE, "container": container, "docker_image_id": binding["id"],
                      "origin": f"http://127.0.0.1:{18980 + offset}", "metrics_origin": f"http://127.0.0.1:{18991 + offset}"}
            pair.private_write(private / (kind + ".json"), json.dumps(config, indent=2))
            pair.docker("create", "--name", container, "--label", f"{pair.LABEL}={owner}", "--network", owner,
                        "--platform", "linux/amd64", "--cpus", "2", "--cpuset-cpus", "0-7",
                        "--memory", "2g", "--memory-swap", "2g", "--pids-limit", "256",
                        "--env-file", str(envfile), "--publish", f"127.0.0.1:{18980 + offset}:8080",
                        "--publish", f"127.0.0.1:{18991 + offset}:9091", "--mount",
                        f"type=bind,source={private / 'provider-key'},destination=/run/synthetic-provider-key,readonly", "--mount",
                        f"type=bind,source={private / 'kubeconfig'},destination=/run/synthetic-kubeconfig,readonly", binding["id"])
            owned.append(container)
            pair.docker("start", container)
            configs[kind] = config
        result["containers_started_utc"] = pair.utc()
        (output / "campaign-start.json").write_text(json.dumps(result, indent=2) + "\n")
        with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
            list(pool.map(lambda item: pair.startup(item[1], output / item[0]), configs.items()))
        result["startup_pass_utc"] = pair.utc()
        time.sleep(12)
        result["measurement"] = measure(configs, output)
        result["outcome"] = "completed"
    except BaseException as error:
        result["failure_type"] = type(error).__name__
        result["failure_location"] = [{"file": Path(frame.filename).name, "line": frame.lineno, "function": frame.name}
                                      for frame in traceback.extract_tb(error.__traceback__)]
        if isinstance(error, subprocess.CalledProcessError):
            result["subcommand_exit_code"] = error.returncode
            pair.private_write(private / "subcommand-failure.log", (error.stdout or "") + (error.stderr or ""))
        raise
    finally:
        result["cleanup"] = []
        for container in reversed(owned):
            try:
                pair.private_write(private / (container + ".log"), pair.docker("logs", container, timeout=10))
            except Exception:
                pass
            pair.remove_owned(container, owner)
            result["cleanup"].append(container)
        if network:
            pair.docker("network", "rm", owner)
        result["ended_utc"] = pair.utc()
        (output / "campaign.json").write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt()))
    try:
        run(args.output)
    except BaseException as error:
        print(json.dumps({"outcome": "incomplete", "failure_type": type(error).__name__}), flush=True)
        raise SystemExit(1)
