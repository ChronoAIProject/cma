"""Stage exact runtime candidates without submitting an Agent Manager rollout."""

from datetime import datetime, timedelta, timezone
import os
from pathlib import Path
import stat
import sys
import time

from cd_target import PRODUCTION, STAGING, name as target_name
from prod_cd_select import release_bounds
from runtime_pointer_publication import READINESS_LANES, promote, qualified_lane
from staging_cd_clock import remaining
from staging_cd_common import Refusal, matches, require
from staging_cd_deploy_result import POINTER_SCHEMA, read as read_component_result
from staging_cd_github import GitHub
from staging_cd_process import run as run_process
from staging_cd_receipts import canonical, sha256
from staging_cd_registry import Registry
from staging_cd_release_tags import local_tag_reader
from staging_cd_set import document, private_document, release_set
from staging_cd_targets import RUNTIME_TARGETS
from staging_cd_watermark import require_released_versions


def run(runner, script, path, request, receipts, started, mode, target, env, child_env, limit, masker):
    runtimes = RUNTIME_TARGETS & receipts.keys()
    enabled = env.get("CD_RUNTIME_POINTER_PUBLICATION_ENABLED") == "true"
    if target == PRODUCTION and runtimes:
        require(enabled and runtimes == {"runtime"}, "Production requires an authorized Agent runtime pointer")
    if enabled and runtimes:
        masker(env.get("CD_RUNTIME_POINTER_TOKEN"))
        require(env.get("CD_RUNTIME_POINTER_TOKEN"), "Runtime pointer publication needs a registry writer")
        apps = {key: receipt for key, receipt in receipts.items() if key not in RUNTIME_TARGETS}
        if not apps:
            return execute(script, path, request, receipts, started, env, child_env, limit=limit)
        return coordinated(runner, script, path, request, receipts, apps, started, env, child_env, limit=limit)
    return runner(script, path, mode, started, child_env)


def coordinated(runner, script, path, request, receipts, apps, started, env, child_env, *, limit):
    """A separately admitted app rollout completes before its runtime pointers may move."""
    target = target_name(env)
    preflight = cohort(script, path, request, path.with_name("pointer-cohort-preflight.json"), child_env,
                       min(40, remaining(started)), target, phase="preflight")
    runtimes = {key: receipt for key, receipt in receipts.items() if key in RUNTIME_TARGETS}
    app_request = private_document(release_set(apps))
    runtime_request = private_document(release_set(runtimes))
    app_path = subrequest(path, "apps", app_request)
    runtime_path = subrequest(path, "runtimes", runtime_request)
    code = runner(script, app_path, "release", started, child_env)
    if code != 0:
        return code if code != 50 else 20
    app_result = read_component_result(app_path.with_name("component-result.json"), app_request, code)
    require(app_result is not None, "Coordinated application deployment lacks a serving receipt")
    remaining(started)
    execute(script, runtime_path, runtime_request, runtimes, started, env, child_env, limit=limit,
            proof=(path, request), expected_lanes=preflight["lanes"])
    runtime_result = read_component_result(runtime_path.with_name("component-result.json"), runtime_request, 0)
    require(runtime_result is not None, "Coordinated runtime deployment lacks a serving receipt")
    write_result(path.with_name("component-result.json"), request,
                 app_result["components"] | runtime_result["components"])
    return 0


def subrequest(original, label, request):
    directory = original.parent / label
    directory.mkdir(mode=0o700)
    path = directory / original.name
    with path.open("x", encoding="utf-8") as stream:
        os.fchmod(stream.fileno(), 0o600)
        stream.write(canonical(request))
    return path


def cohort(script, path, request, output, child_env, timeout, target, *, phase="promotion"):
    reader = script.with_name("runtime_pointer_cohort.py")
    require(reader.is_file() and not reader.is_symlink(), "Runtime pointer cohort reader is unavailable")
    require(run_process([sys.executable, "-B", str(reader), "--release-set", str(path),
                         "--output", str(output), "--read-seconds", str(timeout), "--environment", target,
                         "--phase", phase],
                        child_env, timeout + 2, cwd=script.parents[2], grace_seconds=6) == 0,
            "Runtime pointer cohort could not be verified")
    details = output.lstat()
    require(stat.S_ISREG(details.st_mode) and not details.st_mode & 0o077 and details.st_size <= 8192,
            "Runtime pointer cohort result is invalid")
    value = document(output.read_text(encoding="utf-8"))
    require(isinstance(value, dict) and set(value) == {"schema", "request", "configuration", "pods", "build", "lanes"}
            and value["schema"] == "cma-runtime-pointer-cohort-v2" and value["request"] == sha256(request)
            and matches(r"[0-9a-f]{64}", value["configuration"]) and isinstance(value["pods"], dict)
            and bool(value["pods"]) and all(matches(r"[0-9a-f]{64}", key) and isinstance(counts, list)
                                           and len(counts) == 2
                                           and all(type(count) is int and count >= 0 for count in counts)
                                           for key, counts in value["pods"].items()),
            "Runtime pointer cohort result is unbound")
    build, lanes = value["build"], value["lanes"]
    require(isinstance(build, dict) and set(build) == {"digest", "source"}
            and matches(r"sha256:[0-9a-f]{64}", build["digest"]) and matches(r"[0-9a-f]{40}", build["source"])
            and isinstance(lanes, dict) and "runtime" in lanes and lanes.keys() <= RUNTIME_TARGETS
            and all(isinstance(lane, dict) and set(lane) == {"digest", "revision"}
                    and matches(r"sha256:[0-9a-f]{64}", lane["digest"])
                    and qualified_lane(lane, lane["digest"]) for lane in lanes.values()),
            "Runtime pointer observation is invalid")
    return value


def execute(script, path, request, receipts, started, env, child_env, *, limit, proof=None, expected_lanes=None):
    remaining(started)
    target = target_name(env)
    deadline = time.monotonic() + remaining(started, limit)
    api = GitHub(env.get("CD_RUNTIME_POINTER_READ_TOKEN"), local_tags=local_tag_reader(Path.cwd(), deadline=deadline),
                 deadline=deadline)
    registry = Registry(credential=env.get("CD_RUNTIME_POINTER_TOKEN"), actor=env.get("GITHUB_ACTOR"),
                        deadline=deadline)
    proof_path, proof_request = proof or (path, request)
    baseline = cohort(script, proof_path, proof_request, proof_path.with_name("pointer-cohort-before.json"),
                      child_env, min(40, remaining(started)), target)
    require(receipts.keys() <= baseline["lanes"].keys()
            and (expected_lanes is None or expected_lanes == baseline["lanes"]),
            "Active runtime changed before pointer publication")
    publications = {component: api.tags(component) for component in receipts}
    history = release_bounds(env, Path.cwd())[1] if target == PRODUCTION else api.history()
    attempt = 0
    authorized = set()

    def observe(timeout):
        nonlocal attempt
        attempt += 1
        current = cohort(script, proof_path, proof_request,
                         proof_path.with_name(f"pointer-cohort-observation-{attempt}.json"),
                         child_env, min(40, timeout), target)
        require(all(current[key] == baseline[key] for key in ("configuration", "pods", "build")),
                "Agent Manager cohort changed during pointer promotion")
        require(current["lanes"].keys() == baseline["lanes"].keys(),
                "Runtime release authority changed its configured lanes")
        for component, prior in baseline["lanes"].items():
            active = current["lanes"][component]
            candidate = receipts.get(component)
            require(active == prior or (
                candidate is not None and component in authorized and active["revision"] > prior["revision"]
                and active["digest"] == candidate["digest"]
            ), "Runtime release authority changed outside this deployment")
        return current["lanes"]

    def require_current(candidates):
        if target == PRODUCTION:
            released, current = release_bounds(env, Path.cwd())
            require(released == current.head, "A newer production release owns runtime pointer publication")
            require_released_versions(candidates, current)
        else:
            api.require_current_versions(candidates)

    def fence(candidates):
        observe(remaining(started))
        require_current(candidates)
        if target == PRODUCTION:
            require(datetime.now(timezone(timedelta(hours=8))).hour != 2,
                    "Production runtime pointer writes are paused during the release blackout")
        remaining(started)
        authorized.update(candidates)

    def read_ready(timeout):
        # The publisher has now confirmed every exact pointer, including replays.
        authorized.update(receipts)
        lanes = observe(timeout)
        return {"status": "ready", "runtime_release": {READINESS_LANES[key]: lane for key, lane in lanes.items()}}

    active = promote(receipts, publications, history, registry, read_ready, environment=target, fence=fence,
                     deadline=deadline - 5)
    remaining(started, limit)
    require_current(receipts)
    remaining(started, limit)

    components = {component: {"state": "controller-verified", "reason": "none",
                              "image": row["image"], "source_sha": row["source_sha"]}
                  for component, row in request["components"].items()}
    for component in components:
        lane = active[READINESS_LANES[component]]
        components[component]["pointer"] = {
            "schema": POINTER_SCHEMA, "environment": target, "revision": lane["revision"],
            "build": baseline["build"]["source"], "build_digest": baseline["build"]["digest"],
            "previous": baseline["lanes"][component],
        }
    write_result(path.with_name("component-result.json"), request, components)
    return 0


def write_result(path, request, components):
    result = {"schema": "cma-private-deployment-result-v1", "request": sha256(request), "code": 0,
              "components": components}
    with path.open("x", encoding="utf-8") as stream:
        os.fchmod(stream.fileno(), 0o600)
        stream.write(canonical(result) + "\n")
