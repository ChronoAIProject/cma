"""Stage exact runtime candidates without submitting an Agent Manager rollout."""

from datetime import datetime, timedelta, timezone
import os
from pathlib import Path
import stat
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

from cd_target import PRODUCTION, STAGING, name as target_name
from prod_cd_select import release_bounds
from runtime_pointer_publication import READINESS_LANES, promote, qualified_lane
from staging_cd_clock import remaining
from staging_cd_common import Refusal, matches, require
from staging_cd_deploy_result import read as read_component_result
from staging_cd_github import GitHub
from staging_cd_process import run as run_process
from staging_cd_receipts import canonical, sha256
from staging_cd_registry import NoRedirect, Registry
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
        apps = {key: receipt for key, receipt in receipts.items() if key not in RUNTIME_TARGETS}
        if not apps:
            return execute(script, path, request, receipts, started, env, child_env, limit=limit)
        return coordinated(runner, script, path, request, receipts, apps, started, env, child_env, limit=limit)
    return runner(script, path, mode, started, child_env)


def coordinated(runner, script, path, request, receipts, apps, started, env, child_env, *, limit):
    """A separately admitted app rollout completes before its runtime pointers may move."""
    read_ready = readiness_reader(env.get("CD_RUNTIME_POINTER_READINESS", ""))
    require(env.get("CD_RUNTIME_POINTER_TOKEN") and isinstance(read_ready(min(10, remaining(started))), dict),
            "Runtime pointer publication is unavailable for the coordinated release")
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
    execute(script, runtime_path, runtime_request, runtimes, started, env, child_env, limit=limit, admitted=True)
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


def cohort(script, path, request, output, child_env, timeout, target):
    reader = script.with_name("runtime_pointer_cohort.py")
    require(reader.is_file() and not reader.is_symlink(), "Runtime pointer cohort reader is unavailable")
    require(run_process([sys.executable, "-B", str(reader), "--release-set", str(path),
                         "--output", str(output), "--read-seconds", str(timeout), "--environment", target],
                        child_env, timeout + 2, cwd=script.parents[2], grace_seconds=6) == 0,
            "Runtime pointer cohort could not be verified")
    details = output.lstat()
    require(stat.S_ISREG(details.st_mode) and not details.st_mode & 0o077 and details.st_size <= 8192,
            "Runtime pointer cohort result is invalid")
    value = document(output.read_text(encoding="utf-8"))
    require(isinstance(value, dict) and set(value) == {"schema", "request", "configuration", "pods"}
            and value["schema"] == "cma-runtime-pointer-cohort-v1" and value["request"] == sha256(request)
            and matches(r"[0-9a-f]{64}", value["configuration"]) and isinstance(value["pods"], dict)
            and bool(value["pods"]) and all(matches(r"[0-9a-f]{64}", key) and type(count) is int and count >= 0
                                           for key, count in value["pods"].items()),
            "Runtime pointer cohort result is unbound")
    return value


def readiness_reader(endpoint):
    url = urllib.parse.urlsplit(endpoint)
    require(url.scheme == "https" and url.hostname and not url.username and not url.password
            and url.path == "/ready" and not url.query and not url.fragment,
            "Agent Manager readiness endpoint is invalid")
    opener = urllib.request.build_opener(NoRedirect())

    def read(timeout):
        request = urllib.request.Request(endpoint, headers={"Accept": "application/json", "Cache-Control": "no-store"})
        try:
            with opener.open(request, timeout=min(10, timeout)) as response:
                return document(response.read(65537).decode("utf-8"))
        except urllib.error.HTTPError as error:
            if error.code in {429, 502, 503, 504}:
                return None
            raise
        except (TimeoutError, urllib.error.URLError):
            return None

    return read


def execute(script, path, request, receipts, started, env, child_env, *, limit, admitted=False):
    target = target_name(env)
    deadline = time.monotonic() + remaining(started, limit)
    read_ready = readiness_reader(env.get("CD_RUNTIME_POINTER_READINESS", ""))
    api = GitHub(env.get("CD_RUNTIME_POINTER_READ_TOKEN"), local_tags=local_tag_reader(Path.cwd(), deadline=deadline),
                 deadline=deadline)
    registry = Registry(credential=env.get("CD_RUNTIME_POINTER_TOKEN"), actor=env.get("GITHUB_ACTOR"),
                        deadline=deadline)
    write_limit = limit if admitted else None
    baseline = cohort(script, path, request, path.with_name("pointer-cohort-before.json"), child_env,
                      min(40, remaining(started, write_limit)), target)
    ready = read_ready(min(10, remaining(started, write_limit)))
    require(isinstance(ready, dict) and ready.get("status") == "ready"
            and isinstance(ready.get("runtime_release"), dict)
            and all(isinstance(ready["runtime_release"].get(READINESS_LANES[key]), dict)
                    for key in receipts),
            "Serving Agent Manager readiness is unavailable before publication")
    publications = {component: api.tags(component) for component in receipts}
    history = release_bounds(env, Path.cwd())[1] if target == PRODUCTION else api.history()

    def fence(candidates):
        remaining(started, write_limit)
        if target == PRODUCTION:
            require(datetime.now(timezone(timedelta(hours=8))).hour != 2,
                    "Production runtime pointer writes are paused during the release blackout")
            released, current = release_bounds(env, Path.cwd())
            require(released == current.head, "A newer production release owns runtime pointer publication")
            require_released_versions(candidates, current)
        else:
            api.require_current_versions(candidates)

    promote(receipts, publications, history, registry, read_ready, environment=target, fence=fence,
            deadline=deadline - 45)
    final = cohort(script, path, request, path.with_name("pointer-cohort-after.json"), child_env,
                   min(40, remaining(started, limit)), target)
    require(final["configuration"] == baseline["configuration"] and all(
        count == baseline["pods"][pod] for pod, count in final["pods"].items() if pod in baseline["pods"]
    ) and any(pod in final["pods"] for pod in baseline["pods"]), "Agent Manager cohort changed during pointer promotion")
    ready = read_ready(min(10, remaining(started, limit)))
    require(isinstance(ready, dict) and ready.get("status") == "ready"
            and isinstance(ready.get("runtime_release"), dict)
            and all(qualified_lane(ready["runtime_release"].get(READINESS_LANES[key]), row["digest"])
                    for key, row in receipts.items()), "The qualified runtime ceased to be active")

    components = {component: {"state": "controller-verified", "reason": "none",
                              "image": row["image"], "source_sha": row["source_sha"]}
                  for component, row in request["components"].items()}
    write_result(path.with_name("component-result.json"), request, components)
    return 0


def write_result(path, request, components):
    result = {"schema": "cma-private-deployment-result-v1", "request": sha256(request), "code": 0,
              "components": components}
    with path.open("x", encoding="utf-8") as stream:
        os.fchmod(stream.fileno(), 0o600)
        stream.write(canonical(result) + "\n")
