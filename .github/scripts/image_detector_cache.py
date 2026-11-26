"""Authenticated develop-only transport for bounded layer detector evidence."""
import hashlib
import io
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import time
import zipfile

import ci_retry

REPO = "ChronoAIProject/cma"
WORKFLOW = ".github/workflows/staging-cd.yml"
COMPONENTS = {"am", "eg", "web", "runtime", "runtime-omarchy"}
MAX_BYTES = 64 * 1024 * 1024
PRODUCER_FILES = (
    WORKFLOW, ".github/workflows/staging-release.yml",
    ".github/scripts/image_detector_cache.py", ".github/scripts/staging_cd_publish.py",
    ".github/scripts/image_secret_gate.py", ".github/scripts/image_scanner_policy.py",
    ".github/scripts/image-secret-scanner/SOURCE-MANIFEST.json",
    ".github/scripts/image-content-manifest.json",
)


def require(value):
    if not value:
        raise ValueError("Layer detector cache refused")


def context(env):
    require(env.get("GITHUB_REPOSITORY") == REPO and env.get("GITHUB_EVENT_NAME") == "push"
            and env.get("GITHUB_REF") == "refs/heads/develop" and env.get("CD_COMPONENT") in COMPONENTS)
    require(re.fullmatch(r"[0-9a-f]{40}", env.get("GITHUB_SHA", "")))
    for key in ("GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT"):
        require(re.fullmatch(r"[1-9][0-9]*", env.get(key, "")))


def paths(env):
    context(env)
    root = Path(env["RUNNER_TEMP"])
    return root / "cma-layer-detector-input.json", root / "cma-layer-detector-output/cache.json"


def scanner_options(root, env, mode, packet):
    if env.get("CD_LAYER_SCAN_CACHE") != "enabled":
        return []
    source, destination = paths(env)
    # Both catalog bytes and authentic review pins participate through these
    # files. The scanner additionally binds its binary, rules, limits and code.
    data: dict[str, object] = {name: hashlib.sha256((root / name).read_bytes()).hexdigest() for name in PRODUCER_FILES}
    data.update(component=env["CD_COMPONENT"], mode=mode,
                classifications=hashlib.sha256(packet).hexdigest() if packet is not None else None)
    identity = hashlib.sha256(json.dumps(data, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
    options = ["--detector-cache-context", identity, "--detector-cache-output", str(destination)]
    # A completed earlier scan in this same publication chain is also exact
    # evidence. Failed/incomplete scans never get past the publication gate.
    candidate = destination if destination.exists() else source
    if candidate.exists() or candidate.is_symlink():
        require(candidate.is_file() and not candidate.is_symlink())
        options += ["--detector-cache-input", str(candidate)]
    return options


def github(path, *, binary=False, timeout=10):
    require(path.startswith("repos/" + REPO + "/") and ".." not in path)
    result = subprocess.run(["gh", "api", "--hostname", "github.com", "--method", "GET", path],
                            capture_output=True, timeout=timeout, check=False)
    if result.returncode != 0:
        # The cause marks a transport failure as repeatable; the message stays fixed.
        raise ValueError("Layer detector cache refused") from ci_retry.cli_cause(result.stderr)
    require(len(result.stdout) <= MAX_BYTES)
    return result.stdout if binary else json.loads(result.stdout)


def source_matches(root, source, head, *, deadline=None):
    deadline = time.monotonic() + 10 if deadline is None else deadline
    def remaining():
        value = deadline - time.monotonic()
        require(value > 0)
        return min(5, value)
    result = subprocess.run(["git", "merge-base", "--is-ancestor", source, head], cwd=root,
                            capture_output=True, timeout=remaining(), check=False)
    if result.returncode != 0:
        return False
    for name in PRODUCER_FILES:
        result = subprocess.run(["git", "show", source + ":" + name], cwd=root,
                                capture_output=True, timeout=remaining(), check=False)
        if result.returncode != 0 or result.stdout != (root / name).read_bytes():
            return False
    return True


def validate_producer(run, jobs, artifact, component):
    require(run.get("repository", {}).get("full_name") == REPO
            and run.get("head_repository", {}).get("full_name") == REPO
            and run.get("path") == WORKFLOW and run.get("event") == "push"
            and run.get("head_branch") == "develop" and run.get("status") in {"completed", "in_progress"})
    require(type(run["repository"].get("id")) is int and run["repository"]["id"] > 0
            and run["head_repository"].get("id") == run["repository"]["id"])
    source, run_id, attempt = run.get("head_sha"), run.get("id"), run.get("run_attempt")
    require(isinstance(source, str) and re.fullmatch(r"[0-9a-f]{40}", source)
            and type(run_id) is int and run_id > 0 and type(attempt) is int and attempt > 0)
    require(isinstance(jobs.get("jobs"), list) and jobs.get("total_count") == len(jobs["jobs"]))
    matches = [job for job in jobs["jobs"] if job.get("name") == f"Release {component} / Publish"]
    require(len(matches) == 1)
    job = matches[0]
    require(job.get("conclusion") == "success" and job.get("status") == "completed"
            and job.get("head_sha") == source and job.get("run_id") == run_id
            and job.get("run_attempt") == attempt)
    require(artifact.get("name") == f"cma-layer-detector-{component}-{run_id}-{attempt}"
            and artifact.get("expired") is False and type(artifact.get("id")) is int and artifact["id"] > 0
            and type(artifact.get("size_in_bytes")) is int and 0 < artifact["size_in_bytes"] <= MAX_BYTES
            and artifact.get("workflow_run", {}).get("id") == run_id
            and artifact.get("workflow_run", {}).get("head_sha") == source
            and artifact.get("workflow_run", {}).get("repository_id") == run.get("repository", {}).get("id")
            and artifact.get("workflow_run", {}).get("head_repository_id") == run.get("head_repository", {}).get("id")
            and isinstance(artifact.get("digest"), str) and re.fullmatch(r"sha256:[a-f0-9]{64}", artifact["digest"]))


def unpack(raw, artifact):
    require(len(raw) <= MAX_BYTES and "sha256:" + hashlib.sha256(raw).hexdigest() == artifact["digest"])
    with zipfile.ZipFile(io.BytesIO(raw)) as archive:
        require(archive.namelist() == ["cache.json"])
        info = archive.getinfo("cache.json")
        require(0 < info.file_size <= MAX_BYTES and not info.flag_bits & 1
                and (info.external_attr >> 16) & 0o170000 != 0o120000)
        data = archive.read(info)
    require(isinstance(json.loads(data), dict))
    return data


def restore(env, root, get=github, match_source=source_matches, sleep=time.sleep):
    destination, _ = paths(env)
    require(not destination.exists() and not destination.is_symlink())
    deadline = time.monotonic() + 30

    def read(path, **kwargs):
        # Every read is a GET; transport failures repeat inside the restore deadline.
        def attempt():
            remaining = deadline - time.monotonic()
            require(remaining > 0)
            return get(path, timeout=min(10, remaining), **kwargs)
        return ci_retry.call(attempt, name="detector-cache-read", classify=ci_retry.classify,
                             deadline=deadline, sleep=sleep)

    # A finished component can supply evidence while sibling jobs are still running.
    # Request one extra run because the newest result is usually this consumer.
    listing = read(f"repos/{REPO}/actions/workflows/staging-cd.yml/runs?branch=develop&event=push&per_page=6")
    require(isinstance(listing.get("workflow_runs"), list))
    candidates = [run for run in listing["workflow_runs"][:6] if isinstance(run, dict)
                  and run.get("status") in {"completed", "in_progress"}
                  and str(run.get("id")) != env["GITHUB_RUN_ID"]]
    for run in candidates[:5]:
        if time.monotonic() >= deadline:
            break
        try:
            run_id, attempt = run["id"], run["run_attempt"]
            require(type(run_id) is int and type(attempt) is int and run_id > 0 and attempt > 0)
            prefix = f"repos/{REPO}/actions/runs/{run_id}"
            artifacts = read(prefix + "/artifacts?per_page=100")
            require(isinstance(artifacts.get("artifacts"), list)
                    and artifacts.get("total_count") == len(artifacts["artifacts"]))
            name = f"cma-layer-detector-{env['CD_COMPONENT']}-{run_id}-{attempt}"
            matches = [artifact for artifact in artifacts["artifacts"] if artifact.get("name") == name]
            if not matches:
                continue
            require(len(matches) == 1)
            jobs = read(prefix + f"/attempts/{attempt}/jobs?per_page=100")
            validate_producer(run, jobs, matches[0], env["CD_COMPONENT"])
            require(match_source(root, run["head_sha"], env["GITHUB_SHA"], deadline=deadline))
            raw = read(f"repos/{REPO}/actions/artifacts/{matches[0]['id']}/zip", binary=True)
            data = unpack(raw, matches[0])
            destination.parent.mkdir(parents=True, exist_ok=True)
            # The scanner must see either complete evidence or no cache.
            with tempfile.TemporaryDirectory(prefix="cma-detector-restore-", dir=destination.parent) as temporary:
                candidate = Path(temporary) / "cache.json"
                candidate.write_bytes(data)
                os.link(candidate, destination)
            return {"state": "RESTORED", "producer_run": run_id, "producer_attempt": attempt,
                    "artifact_id": matches[0]["id"], "artifact_digest": matches[0]["digest"]}
        except (ValueError, KeyError, TypeError, OSError, subprocess.SubprocessError, zipfile.BadZipFile):
            continue
    return {"state": "MISS_FULL_SCAN"}


def main():
    try:
        result = restore(os.environ, Path.cwd())
    except (ValueError, KeyError, TypeError, OSError, subprocess.SubprocessError, zipfile.BadZipFile):
        result = {"state": "UNAVAILABLE_FULL_SCAN"}
    print(json.dumps({"event": "layer-detector-cache", **result}, sort_keys=True))


if __name__ == "__main__":
    main()
