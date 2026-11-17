#!/usr/bin/env python3
"""Promote successful develop CI images to the fixed CMA staging apps."""
import copy
import hashlib
import json
import os
import re
import subprocess
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path

REPO = "ChronoAIProject/cma"
SOURCE = f"https://github.com/{REPO}"
SCOPE = "internal:chrono-ai-staging"
API = "https://nyx-api.chrono-ai.fun/api/v1/proxy/s/chrono-cd/api/v1"
TARGETS = (
    ("web", "cma-staging", "cma-web", "web", "101"),
    ("api", "cma-am-staging", "cma-agent-manager", "agent-manager", "10001"),
    ("cmaeg", "cmaeg-staging", "cma-event-gateway", "event-gateway", "10001"),
)


class Refusal(Exception):
    """Only deliberately safe diagnostics may be printed by the entry point."""


def require(condition, message):
    if not condition:
        raise Refusal(message)


def matches(pattern, value):
    return isinstance(value, str) and re.fullmatch(pattern, value) is not None


def digest(value):
    return matches(r"sha256:[0-9a-f]{64}", value)


def command(*args, input_text=None, timeout=600):
    # Child tools never need the NyxID credential; suppress untrusted diagnostics.
    env = {k: v for k, v in os.environ.items() if k != "NYXID_CHRONO_CD_KEY"}
    try:
        result = subprocess.run(args, input=input_text, text=True, capture_output=True,
                                timeout=timeout, env=env, check=False)
    except (OSError, subprocess.TimeoutExpired) as error:
        raise Refusal(f"{args[0]} could not complete within its command budget") from error
    require(result.returncode == 0, f"{args[0]} command failed; output withheld to protect credentials")
    return result.stdout


def github(path):
    return json.loads(command("gh", "api", f"repos/{REPO}/{path}", timeout=60))


def validate_run(run, sha):
    require(run.get("head_sha") == sha and run.get("head_branch") == "develop"
            and run.get("event") == "push" and run.get("conclusion") == "success"
            and run.get("status") == "completed" and run.get("path") == ".github/workflows/ci.yml"
            and run.get("repository", {}).get("full_name") == REPO
            and run.get("head_repository", {}).get("full_name") == REPO,
            "CI must be a successful same-repository develop push for the exact source SHA")
    require(all(type(run.get(k)) is int and run[k] > 0 for k in ("id", "run_attempt")),
            "CI run ID and attempt must be positive integers")
    return run


def select_run(event, requested, api=github):
    head = api("branches/develop")["commit"]["sha"]
    trigger = event.get("workflow_run")
    sha = trigger.get("head_sha") if trigger else requested or head
    require(matches(r"[0-9a-f]{40}", sha), "A full lowercase 40-character source SHA is required")
    if trigger:
        validate_run(trigger, sha)
        require(sha == head, "Superseded develop CI run; refusing an automatic staging rollback")
        run = api(f"actions/runs/{trigger['id']}")
        require(run.get("run_attempt") == trigger["run_attempt"], "CI attempt changed since the event")
    else:
        comparison = api(f"compare/{sha}...{head}")
        require(comparison.get("status") in ("ahead", "identical"), "Requested SHA is not on develop")
        runs = api(f"actions/workflows/ci.yml/runs?branch=develop&event=push&status=success&head_sha={sha}&per_page=100")
        require(runs.get("workflow_runs"), "No successful develop push CI exists for the requested SHA")
        run = api(f"actions/runs/{runs['workflow_runs'][0]['id']}")
    return validate_run(run, sha)


def verify_image(folder, target, run, run_command=command):
    component, _, _, _, uid = target
    metadata = json.loads((folder / "metadata.json").read_text())
    sha = run["head_sha"]
    archive = folder / f"cma-{component}-linux-amd64.docker.tar.gz"
    expected = {
        "schema": "cma-ci-native-image-v1", "component": component, "source": SOURCE,
        "sourceRevision": sha, "platform": "linux/amd64", "archiveFormat": "docker-save+gzip",
        "archive": archive.name, "testedReference": f"cma-{component}:ci",
        "workflow": {"repository": REPO, "runId": str(run["id"]),
                     "runAttempt": str(run["run_attempt"]), "event": "push", "ref": "refs/heads/develop"},
    }
    require(all(metadata.get(k) == v for k, v in expected.items()), f"{component}: CI artifact provenance differs")
    require(digest(metadata.get("imageId")), f"{component}: invalid image ID")
    require(matches(r"[0-9a-f]{40}", metadata.get("sourceTree")), f"{component}: invalid source tree")
    checksum = hashlib.sha256()
    with archive.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            checksum.update(chunk)
    require(archive.stat().st_size == metadata.get("archiveBytes")
            and checksum.hexdigest() == metadata.get("archiveSha256"), f"{component}: archive checksum/size differs")
    run_command("docker", "image", "load", "--input", str(archive))
    inspected = json.loads(run_command("docker", "image", "inspect", metadata["imageId"]))
    require(isinstance(inspected, list) and len(inspected) == 1, f"{component}: expected one image")
    image = inspected[0]
    config = image.get("Config") or {}
    labels = config.get("Labels") or {}
    require(image.get("Id") == metadata["imageId"] and image.get("Os") == "linux"
            and image.get("Architecture") == "amd64", f"{component}: expected immutable linux/amd64 image")
    require(config.get("User") in (uid, f"{uid}:{uid}"), f"{component}: expected numeric USER {uid}")
    require(labels.get("org.opencontainers.image.source") == SOURCE
            and labels.get("org.opencontainers.image.revision") == sha,
            f"{component}: image source/revision labels differ")
    return metadata["imageId"]


def download_images(run, root, run_command=command, api=github):
    images = {}
    artifacts = []
    page = 1
    while True:
        batch = api(f"actions/runs/{run['id']}/artifacts?per_page=100&page={page}")["artifacts"]
        artifacts.extend(batch)
        if len(batch) < 100:
            break
        page += 1
    for target in TARGETS:
        component = target[0]
        name = f"ci-native-image-{component}-{run['head_sha']}-{run['id']}-{run['run_attempt']}"
        found = [a for a in artifacts if a.get("name") == name and not a.get("expired")]
        require(len(found) == 1, f"{component}: exact CI image artifact missing, expired or ambiguous (retention is one day)")
        folder = root / component
        run_command("gh", "run", "download", str(run["id"]), "--repo", REPO,
                    "--name", name, "--dir", str(folder))
        images[component] = verify_image(folder, target, run, run_command)
        # Large archives need not coexist after Docker has loaded their bytes.
        (folder / f"cma-{component}-linux-amd64.docker.tar.gz").unlink()
        print(f"Verified {component}: linux/amd64, numeric USER, source SHA and CI artifact checksum")
    return images


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None  # Never forward the API key to a redirect destination.


class ChronoCD:
    def __init__(self, key):
        require(key and not any(c.isspace() for c in key), "NYXID_CHRONO_CD_KEY is missing or malformed")
        self.key = key
        self.opener = urllib.request.build_opener(NoRedirect)

    def request(self, app, method="GET", body=None, generation=None, status=False, timeout=60):
        require(app in {t[1] for t in TARGETS}, "Unknown staging app")
        require(method in ("GET", "PUT") and not (status and method != "GET"), "Unsupported staging operation")
        url = f"{API}/projects/cma/apps/{app}{'/status' if status else ''}?org_id={SCOPE}"
        headers = {"X-API-Key": self.key, "Accept": "application/json"}
        if generation is not None:
            headers["If-Match"] = str(generation)
        data = None if body is None else json.dumps(body).encode()
        if data is not None:
            headers["Content-Type"] = "application/json"
        request = urllib.request.Request(url, data=data, headers=headers, method=method)
        try:
            response = self.opener.open(request, timeout=timeout)
        except urllib.error.HTTPError as error:
            response = error
        except (OSError, urllib.error.URLError) as error:
            raise Refusal(f"{app}: chrono-cd request failed; no automatic mutation retry") from error
        with response:
            code = response.code
            require(code != 403, "NyxID denied chrono-cd (403): grant the organization API key permission "
                    "chrono-cd:internal:chrono-ai-staging; deployment stopped")
            require(code != 409, f"{app}: generation conflict (409); inspect concurrent changes before retrying")
            require(code == 200 or (code == 422 and method == "PUT"), f"{app}: chrono-cd HTTP {code}; response withheld")
            result = json.load(response)
        require(isinstance(result, dict), f"{app}: malformed chrono-cd response")
        if code == 422:
            require(app != "cma-staging" and result.get("error") == "missing_secret_keys",
                    f"{app}: unexpected chrono-cd validation failure")
            # The current API calls this field 'project'; re-read after PUT as well.
            return result["project"]
        return result


def validate_app(view, target, reference=None, generation=None):
    _, app, service, _, _ = target
    require(view.get("name") == app and view.get("project") == "cma" and view.get("owner") == SCOPE
            and view.get("namespace") == "chrono-ai-staging", f"{app}: staging app identity differs")
    require(type(view.get("generation")) is int and view["generation"] > 0, f"{app}: invalid generation")
    services = view.get("spec", {}).get("services", {})
    require(service in services, f"{app}: staging service is absent")
    if reference is not None:
        require(services[service].get("image") == reference and view["generation"] == generation,
                f"{app}: stored image or generation changed; refusing stale rollout evidence")
    return view


def replacement_spec(view, target, reference):
    validate_app(view, target)
    require(digest(reference.rsplit("@", 1)[-1]), "Replacement image must be pinned by digest")
    body = copy.deepcopy(view["spec"])
    # App-view fields (generation, status, URL, domain, secret inventories) are
    # outside spec. image_digest is the server-resolved, read-only image field.
    for service in body["services"].values():
        service.pop("image_digest", None)
    body["services"][target[2]]["image"] = reference
    return body


def missing_secrets(view, status, target):
    _, app, service, _, _ = target
    if app == "cma-staging" or status.get("phase") != "error":
        return False
    declared = view["spec"]["services"][service].get("secret_keys", [])
    present = view.get("secret_keys_present", {}).get(service, [])
    missing = [key for key in declared if key not in present]
    last = status.get("last_reconcile") or {}
    # chrono-cd emits exactly 'service: KEY, KEY' without a step for missing keys.
    # A phrase match would also excuse unrelated failures or pod diagnostics.
    services = status.get("services", {})
    return (bool(missing) and last.get("ok") is False and not last.get("step")
            and last.get("error") == f"{service}: {', '.join(missing)}"
            and view.get("status", {}).get("last_reconcile") == last
            and all(s.get("phase") != "degraded" for s in services.values()))


def publish(images, sha, rows, run_command=command):
    try:
        run_command("docker", "login", "ghcr.io", "--username", os.environ["GITHUB_ACTOR"],
                    "--password-stdin", input_text=os.environ["GH_TOKEN"])
        for target in TARGETS:
            component, app, _, package, _ = target
            repository = f"ghcr.io/chronoaiproject/cma-staging-{package}"
            reference = f"{repository}:{sha}"
            run_command("docker", "image", "tag", images[component], reference)
            try:
                run_command("docker", "image", "push", reference)
            except Refusal as error:
                raise Refusal(f"{app}: GHCR push failed; owner must grant this repository write access "
                              "in each staging package's Settings > Manage Actions access") from error
            image = json.loads(run_command("docker", "image", "inspect", reference))[0]
            refs = [r for r in image.get("RepoDigests", []) if r.startswith(repository + "@")]
            require(len(refs) == 1 and digest(refs[0].split("@", 1)[1]), f"{app}: missing published digest")
            manifest = json.loads(run_command("docker", "manifest", "inspect", refs[0]))
            require(manifest.get("config", {}).get("digest") == images[component], f"{app}: registry config differs from CI")
            rows[app]["image"] = refs[0]
            print(f"Published {app}: {refs[0]}")
    finally:
        run_command("docker", "logout", "ghcr.io", timeout=30)


def rollout(client, targets, rows, dry_run, clock=time.monotonic, sleep=time.sleep):
    pending = list(targets)
    deadline = clock() + 600
    while pending:
        for target in list(pending):
            app = target[1]
            remaining = deadline - clock()
            require(remaining > 0, "Staging status did not settle within ten minutes")
            view = client.request(app, timeout=min(30, remaining))
            validate_app(view, target, None if dry_run else rows[app]["image"], rows[app].get("generation"))
            remaining = deadline - clock()
            require(remaining > 0, "Staging status did not settle within ten minutes")
            status = client.request(app, status=True, timeout=min(30, remaining))
            phase = status.get("phase")
            require(phase in ("ready", "progressing", "degraded", "error", "deleting"), f"{app}: unknown status phase")
            rows[app]["phase"] = phase
            if missing_secrets(view, status, target):
                rows[app]["phase"] = "error (expected: missing secret keys; not ready)"
                print(f"::warning::{app}: staging secret keys are missing; image stored but app is not ready")
                pending.remove(target)
            elif phase == "ready":
                pending.remove(target)
            else:
                require(phase == "progressing", f"{app}: rollout phase is {phase}")
        if pending:
            remaining = deadline - clock()
            require(remaining > 0, "Staging status did not settle within ten minutes")
            sleep(min(15, remaining))


def promote(client, images, sha, rows, dry_run, publisher=publish):
    # Detect permission and scope failures before any package publication.
    before = {t[1]: validate_app(client.request(t[1]), t) for t in TARGETS}
    if not dry_run:
        publisher(images, sha, rows)
        for target in TARGETS:
            app = target[1]
            rows[app]["phase"] = "PUT requested"
            result = client.request(app, method="PUT", generation=before[app]["generation"],
                                    body=replacement_spec(before[app], target, rows[app]["image"]))
            rows[app]["generation"] = before[app]["generation"] + 1
            validate_app(result, target, rows[app]["image"], rows[app]["generation"])
    rollout(client, TARGETS, rows, dry_run)


def summary(run, rows, dry_run, failure):
    lines = ["## CMA staging CD", f"Mode: {'dry run (no push or PUT)' if dry_run else 'deploy'}",
             f"Source SHA: `{run.get('head_sha', 'unresolved')}`",
             f"CI run: {run.get('id', 'unresolved')}; attempt: {run.get('run_attempt', 'unresolved')}", "",
             "| App | Published image digest | Phase |", "| --- | --- | --- |"]
    for app, row in rows.items():
        lines.append(f"| {app} | {row['image']} | {row['phase']} |")
    if failure:
        lines.extend(["", f"Failed: {failure}"])
    with open(os.environ["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as output:
        output.write("\n".join(lines) + "\n")


def main():
    run = {}
    rows = {t[1]: {"image": "not published", "phase": "not checked"} for t in TARGETS}
    failure = None
    dry_run = os.environ.get("DRY_RUN", "true") != "false"
    try:
        for name in ("GH_TOKEN", "NYXID_CHRONO_CD_KEY"):
            value = os.environ.get(name, "")
            if value:
                escaped = value.replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")
                print(f"::add-mask::{escaped}", flush=True)
        require(os.environ.get("GITHUB_REPOSITORY") == REPO, "Staging CD is restricted to its source repository")
        require(os.environ.get("GITHUB_EVENT_NAME") in ("workflow_run", "workflow_dispatch"), "Unsupported CD event")
        event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text())
        require(("workflow_run" in event) == (os.environ["GITHUB_EVENT_NAME"] == "workflow_run"), "CD event payload differs")
        require(dry_run or os.environ.get("GITHUB_REF") == "refs/heads/develop", "Writes require the develop branch")
        run = select_run(event, os.environ.get("REQUESTED_SHA", ""))
        print(f"Selected CI {run['id']} attempt {run['run_attempt']} for {run['head_sha']}")
        with tempfile.TemporaryDirectory(prefix="cma-staging-", dir=os.environ.get("RUNNER_TEMP")) as directory:
            images = download_images(run, Path(directory))
            promote(ChronoCD(os.environ.get("NYXID_CHRONO_CD_KEY", "")), images, run["head_sha"], rows, dry_run)
    except Refusal as error:
        failure = str(error)
    except (OSError, ValueError, KeyError, TypeError, AttributeError, IndexError):
        # JSON, schema and tool failures can contain response bodies or secrets.
        failure = "Malformed response, artifact or execution context; deployment stopped (details withheld)"
    finally:
        summary(run, rows, dry_run, failure)
    if failure:
        print(f"::error::{failure}")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
