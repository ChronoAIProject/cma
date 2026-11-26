"""Qualify CI image artifacts against exact merged source and build inputs."""
import hashlib
import json
import os
import re
import subprocess
import tempfile
import time
import zipfile
import zlib
from pathlib import Path

import ci_retry
from deployables import registry as deployable_registry
from staging_cd_build import command, inspect
from staging_cd_common import Refusal, matches, require
from staging_cd_contract import from_source
from staging_cd_versions import DEPLOYABLES, REPOSITORY

MAX_ARTIFACT = 8 * 1024 * 1024 * 1024
# Optional discovery, download and conversion share a budget with each other.
# The reserved-source build remains available when that budget expires.
REUSE_SECONDS = 900
DOWNLOAD_SECONDS = 600
QUEUE_EVENT = "merge_group"
POST_MERGE_WORKFLOW = ".github/workflows/ci.yml"
PR_IMAGE_WORKFLOWS = {
    "api": ".github/workflows/ci-pr-am.yml",
    "runtime": ".github/workflows/ci-pr-am.yml",
    "runtime-omarchy": ".github/workflows/ci-pr-am.yml",
    "cmaeg": ".github/workflows/ci-pr-eg.yml",
    "web": ".github/workflows/ci-pr-web.yml",
}


def manifest_miss(metadata, expected, run):
    if not isinstance(metadata, dict) or metadata.get("schema") != "cma-ci-native-image-v1":
        return "manifest:schema"
    fields = {
        "component": deployable_registry()[DEPLOYABLES[expected["component"]]]["component"],
        "sourceRevision": expected["source"],
        "sourceTree": expected["tree"],
        "platform": "linux/amd64",
        "buildFingerprint": expected["build"],
        "versionLabel": expected["version"],
    }
    for field, value in fields.items():
        if metadata.get(field) != value:
            return "manifest:" + field
    for field in ("archive", "archiveSha256", "archiveBytes"):
        if field not in metadata:
            return "manifest:" + field
    workflow = metadata.get("workflow")
    event = run.get("event")
    if (not isinstance(workflow, dict) or workflow.get("repository") != REPOSITORY
            or event not in ("pull_request", QUEUE_EVENT) or workflow.get("event") != event
            or (event == QUEUE_EVENT and workflow.get("ref") != "refs/heads/" + str(run.get("head_branch")))
            or workflow.get("runId") != str(run["id"]) or workflow.get("runAttempt") != str(run["run_attempt"])):
        return "manifest:workflow"
    return None


def qualifies(metadata, expected, run):
    return manifest_miss(metadata, expected, run) is None


def download(artifact, destination, token, *, deadline=None, retry=None):
    require(type(artifact.get("id")) is int and artifact["id"] > 0
            and type(artifact.get("size_in_bytes")) is int and 0 < artifact["size_in_bytes"] <= MAX_ARTIFACT
            and matches(r"sha256:[0-9a-f]{64}", artifact.get("digest")), "CI artifact identity is incomplete")
    env = {key: value for key, value in os.environ.items() if key in ("PATH", "HOME", "LANG", "LC_ALL")}
    env.update(GH_TOKEN=token, GH_HOST="github.com", GH_PROMPT_DISABLED="1")

    policy = ci_retry.bounded(retry, deadline)

    def attempt():
        timeout = ci_retry.attempt_timeout(DOWNLOAD_SECONDS, deadline, policy.get("clock", time.monotonic))
        if timeout is None:
            raise Refusal("CI artifact download budget expired") from ci_retry.TransientError()
        stream = destination.open("xb")  # never adopts a file this download did not create
        try:
            with stream:
                result = subprocess.run(["gh", "api", f"repos/{REPOSITORY}/actions/artifacts/{artifact['id']}/zip"],
                                        env=env, stdout=stream, stderr=subprocess.PIPE, timeout=timeout, check=False)
            if result.returncode != 0:
                raise Refusal("CI artifact download failed") from ci_retry.cli_cause(result.stderr)
        except BaseException:
            destination.unlink(missing_ok=True)  # only this attempt's partial bytes
            raise

    ci_retry.call(attempt, name="artifact-download", classify=ci_retry.classify, **policy)
    require(0 < destination.stat().st_size <= MAX_ARTIFACT, "CI artifact download failed")
    digest = hashlib.sha256()
    with destination.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    require("sha256:" + digest.hexdigest() == artifact["digest"], "CI artifact checksum differs")


def successful_runs(api, event, head, deadline, *, component=None):
    endpoint = "actions/workflows/ci.yml/runs" if component is None else "actions/runs"
    paths = {POST_MERGE_WORKFLOW}
    if component is not None:
        paths.add(PR_IMAGE_WORKFLOWS[component])
    data = api.request(f"{endpoint}?event={event}&head_sha={head}&status=success&per_page=100",
                       deadline=deadline)
    require(isinstance(data, dict) and isinstance(data.get("workflow_runs"), list), "CI run inventory is malformed")
    return [run for run in data["workflow_runs"] if isinstance(run, dict) and run.get("event") == event
            and run.get("status") == "completed" and run.get("conclusion") == "success"
            and run.get("head_sha") == head and run.get("path") in paths
            and type(run.get("id")) is int and type(run.get("run_attempt")) is int
            and isinstance(run.get("repository"), dict) and run["repository"].get("full_name") == REPOSITORY]


def queue_run(expected, pr, api, deadline):
    """The develop merge-queue run whose group commit became this exact source."""
    # Queued fork code runs in this repository's context. Its CI bytes are
    # never adopted: a fork-originated source is always rebuilt here.
    repository, base = pr["head"].get("repo"), pr.get("base")
    if (not isinstance(repository, dict) or repository.get("full_name") != REPOSITORY
            or not isinstance(base, dict) or base.get("ref") != "develop"):
        return None
    branch = rf"gh-readonly-queue/develop/pr-{expected['pr']}-[0-9a-f]{{40}}"
    runs = [run for run in successful_runs(api, QUEUE_EVENT, expected["source"], deadline)
            if isinstance(run.get("head_repository"), dict)
            and run["head_repository"].get("full_name") == REPOSITORY
            and matches(branch, run.get("head_branch"))]
    return max(runs, key=lambda item: (item["id"], item["run_attempt"])) if runs else None


def pull_request_run(expected, pr, api, deadline):
    component = deployable_registry()[DEPLOYABLES[expected["component"]]]["component"]
    runs = successful_runs(api, "pull_request", pr["head"]["sha"], deadline, component=component)
    return max(runs, key=lambda item: (item["id"], item["run_attempt"])) if runs else None


def native_artifact(expected, run, api, deadline):
    listing = api.request(f"actions/runs/{run['id']}/artifacts?per_page=100", deadline=deadline)
    require(isinstance(listing, dict) and isinstance(listing.get("artifacts"), list)
            and listing.get("total_count") == len(listing["artifacts"]), "CI artifact inventory is incomplete")
    component = deployable_registry()[DEPLOYABLES[expected["component"]]]["component"]
    # The producer names the checkout it tested. A PR merge preview can have
    # a different commit from the final squash even when their trees match.
    name = f"ci-native-image-{component}-{expected['source']}-{run['id']}-{run['run_attempt']}"
    artifacts = [item for item in listing["artifacts"] if isinstance(item, dict)
                 and item.get("name") == name and item.get("expired") is False]
    require(len(artifacts) <= 1, "Native PR artifact is ambiguous")
    return artifacts[0] if artifacts else None


def fetch(expected, api, archive, token, downloader, deadline):
    """Download the native image of the newest successful CI run for this source.

    A merge-queue run tested the exact develop commit, so it is preferred. A PR
    run tested its own merge preview; its manifest qualifies only if it matches.
    """
    pr = api.request("pulls/" + str(expected["pr"]), deadline=deadline)
    require(isinstance(pr, dict) and pr.get("merged") is True and pr.get("merge_commit_sha") == expected["source"]
            and isinstance(pr.get("head"), dict) and matches(r"[0-9a-f]{40}", pr["head"].get("sha")),
            "PR artifact source cannot be established")
    lookups = (lambda: queue_run(expected, pr, api, deadline),
               lambda: pull_request_run(expected, pr, api, deadline))
    found_run = False
    for lookup in lookups:
        run = lookup()
        if run is None:
            continue
        found_run = True
        artifact = native_artifact(expected, run, api, deadline)
        if artifact is not None:
            downloader(artifact, archive, token, deadline=deadline)
            return run, None
    return None, "no-artifact" if found_run else "no-ci-run"


def try_reuse(expected, directory, api, token, downloader=download, runner=command, *,
              clock=time.monotonic, on_miss=None, source_root=None):
    def miss(reason):
        if on_miss is not None:
            on_miss(reason)
        return None

    deadline = clock() + REUSE_SECONDS
    with tempfile.TemporaryDirectory(prefix="ci-candidate-", dir=directory) as scratch:
        scratch = Path(scratch)
        archive = scratch / "candidate.zip"
        try:
            run, reason = fetch(expected, api, archive, token, downloader, deadline)
        except (Refusal, OSError, subprocess.SubprocessError, json.JSONDecodeError) as error:
            reason = "unreachable" if ci_retry.classify(error) == ci_retry.TRANSIENT else "candidate-unavailable"
            return miss(reason)
        if run is None:
            return miss(reason)

        try:
            with zipfile.ZipFile(archive) as source:
                entries = source.infolist()
                require(len(entries) <= 3 and len({entry.filename for entry in entries}) == len(entries)
                        and all(not Path(entry.filename).is_absolute() and ".." not in Path(entry.filename).parts
                                and not entry.flag_bits & 1
                                and entry.compress_type in (zipfile.ZIP_STORED, zipfile.ZIP_DEFLATED)
                                and entry.file_size <= MAX_ARTIFACT for entry in entries),
                        "Native PR artifact entries are unsafe")
                metadata_entry = source.getinfo("metadata.json")
                require(metadata_entry.file_size <= 16384, "Native PR metadata exceeds limit")
                metadata = json.loads(source.read(metadata_entry))
                reason = manifest_miss(metadata, expected, run)
                if reason is not None:
                    return miss(reason)
                filename = metadata["archive"]
                require(isinstance(filename, str) and re.fullmatch(r"cma-[a-z-]+-linux-amd64\.docker\.tar\.gz", filename)
                        and matches(r"[0-9a-f]{64}", metadata["archiveSha256"]), "Native PR archive name or digest is invalid")
                entry = source.getinfo(filename)
                require(type(metadata["archiveBytes"]) is int and 0 < entry.file_size == metadata["archiveBytes"],
                        "Native PR archive length differs")
                target = scratch / "qualified.docker.tar.gz"
                digest, length = hashlib.sha256(), 0
                with source.open(entry) as incoming, target.open("xb") as outgoing:
                    for chunk in iter(lambda: incoming.read(1024 * 1024), b""):
                        require(clock() < deadline, "CI image extraction budget expired")
                        length += len(chunk)
                        require(length <= MAX_ARTIFACT, "Native PR archive exceeds limit")
                        digest.update(chunk)
                        outgoing.write(chunk)
                require(length == metadata["archiveBytes"] and digest.hexdigest() == metadata["archiveSha256"],
                        "Native PR archive checksum differs")
            output = scratch / "image.oci.tar"

            def bounded_runner(*args, timeout, **options):
                remaining = ci_retry.attempt_timeout(timeout, deadline, clock)
                require(remaining is not None, "CI image conversion budget expired")
                return runner(*args, timeout=remaining, **options)

            # Conversion happens before scanning. It changes only the transport.
            bounded_runner(["skopeo", "copy", "--format", "oci", "docker-archive:" + str(target), "oci-archive:" + str(output)],
                           timeout=600)
            compatibility = from_source(expected["component"], expected["source"], source_root)
            inspect(output, expected, bounded_runner, allow_qualified=True, compatibility=compatibility)
            published = directory / output.name
            os.link(output, published)
            return published
        except (OSError, KeyError, ValueError, EOFError, subprocess.SubprocessError, zipfile.BadZipFile,
                zipfile.LargeZipFile, zlib.error):
            return miss("candidate-invalid")
