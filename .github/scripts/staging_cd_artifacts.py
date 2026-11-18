"""Qualify PR image artifacts against exact merged source and build inputs."""
import hashlib
import json
import os
import re
import subprocess
import zipfile
from pathlib import Path

from deployables import registry as deployable_registry
from staging_cd_build import command, inspect
from staging_cd_common import matches, require
from staging_cd_versions import DEPLOYABLES, REPOSITORY

MAX_ARTIFACT = 8 * 1024 * 1024 * 1024


def qualifies(metadata, expected, run):
    # The original native-image manifest has no Dockerfile/build-argument
    # binding. It cannot prove equivalent inputs and therefore falls back to
    # one build, even if its Git tree happens to match the merged source.
    required = {"schema", "component", "sourceRevision", "sourceTree", "platform", "versionLabel",
                "buildFingerprint", "archive", "archiveSha256", "archiveBytes", "workflow"}
    if not isinstance(metadata, dict) or not required.issubset(metadata):
        return False
    workflow = metadata["workflow"]
    return (metadata["schema"] == "cma-ci-native-image-v1"
            and metadata["component"] == deployable_registry()[DEPLOYABLES[expected["component"]]]["component"]
            and metadata["sourceRevision"] == expected["source"] and metadata["sourceTree"] == expected["tree"]
            and metadata["platform"] == "linux/amd64" and metadata["versionLabel"] == expected["version"]
            and metadata["buildFingerprint"] == expected["build"] and isinstance(workflow, dict)
            and workflow.get("repository") == REPOSITORY and workflow.get("event") == "pull_request"
            and workflow.get("runId") == str(run["id"]) and workflow.get("runAttempt") == str(run["run_attempt"]))


def download(artifact, destination, token):
    require(type(artifact.get("id")) is int and artifact["id"] > 0
            and type(artifact.get("size_in_bytes")) is int and 0 < artifact["size_in_bytes"] <= MAX_ARTIFACT
            and matches(r"sha256:[0-9a-f]{64}", artifact.get("digest")), "CI artifact identity is incomplete")
    env = {key: value for key, value in os.environ.items() if key in ("PATH", "HOME", "LANG", "LC_ALL")}
    env.update(GH_TOKEN=token, GH_HOST="github.com", GH_PROMPT_DISABLED="1")
    with destination.open("xb") as stream:
        result = subprocess.run(["gh", "api", f"repos/{REPOSITORY}/actions/artifacts/{artifact['id']}/zip"],
                                env=env, stdout=stream, stderr=subprocess.PIPE, timeout=600, check=False)
    require(result.returncode == 0 and 0 < destination.stat().st_size <= MAX_ARTIFACT,
            "CI artifact download failed")
    digest = hashlib.sha256()
    with destination.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    require("sha256:" + digest.hexdigest() == artifact["digest"], "CI artifact checksum differs")


def try_reuse(expected, directory, api, token, downloader=download, runner=command):
    pr = api.request("pulls/" + str(expected["pr"]))
    require(pr.get("merged") is True and pr.get("merge_commit_sha") == expected["source"]
            and isinstance(pr.get("head"), dict) and matches(r"[0-9a-f]{40}", pr["head"].get("sha")),
            "PR artifact source cannot be established")
    head = pr["head"]["sha"]
    data = api.request("actions/workflows/ci.yml/runs?event=pull_request&head_sha=" + head + "&status=success&per_page=100")
    require(isinstance(data, dict) and isinstance(data.get("workflow_runs"), list), "CI run inventory is malformed")
    runs = [run for run in data["workflow_runs"] if run.get("event") == "pull_request"
            and run.get("status") == "completed" and run.get("conclusion") == "success"
            and run.get("head_sha") == head and run.get("path") == ".github/workflows/ci.yml"
            and type(run.get("id")) is int and type(run.get("run_attempt")) is int
            and run.get("repository", {}).get("full_name") == REPOSITORY]
    if not runs:
        return None
    run = max(runs, key=lambda item: (item["id"], item["run_attempt"]))
    listing = api.request(f"actions/runs/{run['id']}/artifacts?per_page=100")
    require(isinstance(listing, dict) and isinstance(listing.get("artifacts"), list)
            and listing.get("total_count") == len(listing["artifacts"]), "CI artifact inventory is incomplete")
    component = deployable_registry()[DEPLOYABLES[expected["component"]]]["component"]
    pattern = rf"ci-native-image-{re.escape(component)}-[0-9a-f]{{40}}-{run['id']}-{run['run_attempt']}"
    artifacts = [item for item in listing["artifacts"] if matches(pattern, item.get("name")) and item.get("expired") is False]
    if not artifacts:
        return None
    require(len(artifacts) == 1, "Native PR artifact is ambiguous")
    archive = directory / "candidate.zip"
    downloader(artifacts[0], archive, token)
    with zipfile.ZipFile(archive) as source:
        entries = source.infolist()
        require(len(entries) <= 3 and len({entry.filename for entry in entries}) == len(entries)
                and all(not Path(entry.filename).is_absolute() and ".." not in Path(entry.filename).parts
                        and entry.file_size <= MAX_ARTIFACT for entry in entries), "Native PR artifact entries are unsafe")
        metadata_entry = source.getinfo("metadata.json")
        require(metadata_entry.file_size <= 16384, "Native PR metadata exceeds limit")
        metadata = json.loads(source.read(metadata_entry))
        if not qualifies(metadata, expected, run):
            return None
        filename = metadata["archive"]
        require(isinstance(filename, str) and re.fullmatch(r"cma-[a-z-]+-linux-amd64\.docker\.tar\.gz", filename)
                and matches(r"[0-9a-f]{64}", metadata["archiveSha256"]), "Native PR archive name or digest is invalid")
        target = directory / "qualified.docker.tar.gz"
        digest, length = hashlib.sha256(), 0
        with source.open(filename) as incoming, target.open("xb") as outgoing:
            for chunk in iter(lambda: incoming.read(1024 * 1024), b""):
                length += len(chunk)
                require(length <= MAX_ARTIFACT, "Native PR archive exceeds limit")
                digest.update(chunk)
                outgoing.write(chunk)
        require(length == metadata["archiveBytes"] and digest.hexdigest() == metadata["archiveSha256"],
                "Native PR archive checksum differs")
    output = directory / "image.oci.tar"
    # Conversion happens before scanning. It changes the transport format,
    # never the compiled image or its source/build inputs.
    runner(["skopeo", "copy", "--format", "oci", "docker-archive:" + str(target), "oci-archive:" + str(output)], timeout=600)
    inspect(output, expected, runner, allow_qualified=True)
    return output
