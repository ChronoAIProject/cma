#!/usr/bin/env python3
"""Export the already tested native Docker image; never build, tag or publish."""
import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent / "ci_tracks"))
from recipe import resolve


def require(condition, message):
    if not condition:
        raise ValueError(message)


def output(*command):
    return subprocess.check_output(command, text=True).strip()


def export(component, revision, destination):
    require(re.fullmatch(r"[0-9a-f]{40}", revision), "full source revision required")
    require(output("git", "rev-parse", "HEAD") == revision, "checkout revision differs")
    subprocess.run(["git", "diff", "--quiet", "HEAD", "--"], check=True)
    source_tree = output("git", "rev-parse", "HEAD^{tree}")
    name = resolve(Path(__file__).resolve().parents[2], component)["name"]
    reference = f"{name}:ci"
    inspected = json.loads(output("docker", "image", "inspect", reference))
    require(isinstance(inspected, list) and len(inspected) == 1, "one native image required")
    image = inspected[0]
    image_id = image.get("Id", "")
    require(re.fullmatch(r"sha256:[0-9a-f]{64}", image_id), "immutable image ID required")
    require(image.get("Os") == "linux" and image.get("Architecture") == "amd64", "native linux/amd64 image required")
    labels = image.get("Config", {}).get("Labels") or {}
    source = "https://github.com/ChronoAIProject/cma"
    require(labels.get("org.opencontainers.image.source") == source, "image source differs")
    require(labels.get("org.opencontainers.image.revision") == revision, "image revision differs")
    version = labels.get("org.opencontainers.image.version")
    require(isinstance(version, str) and version.strip(), "image version label required")
    runtime_contract = labels.get("dev.chrono.cma.runtime-contract")
    if component in ("runtime", "runtime-omarchy"):
        require(runtime_contract == "3", "runtime contract differs")
    repository = os.environ.get("GITHUB_REPOSITORY", "")
    run_id = os.environ.get("GITHUB_RUN_ID", "")
    attempt = os.environ.get("GITHUB_RUN_ATTEMPT", "")
    event = os.environ.get("GITHUB_EVENT_NAME", "")
    ref = os.environ.get("GITHUB_REF", "")
    require(repository == "ChronoAIProject/cma", "CI repository differs")
    require(re.fullmatch(r"[1-9][0-9]*", run_id) and re.fullmatch(r"[1-9][0-9]*", attempt), "CI run identity required")
    verification = (event in ("schedule", "workflow_dispatch") and ref == "refs/heads/develop"
                    and component in ("api", "runtime", "runtime-omarchy"))
    require(event in ("push", "pull_request", "merge_group") or verification, "CI event identity required")

    destination.mkdir(mode=0o700, parents=True, exist_ok=False)
    archive = destination / f"cma-{component}-linux-amd64.docker.tar.gz"
    try:
        # Save the inspected immutable ID, so a mutable local tag cannot redirect export.
        with archive.open("xb") as target:
            with gzip.GzipFile(filename="", mode="wb", compresslevel=1, fileobj=target, mtime=0) as compressed:
                with subprocess.Popen(["docker", "image", "save", image_id], stdout=subprocess.PIPE) as process:
                    shutil.copyfileobj(process.stdout, compressed, length=1024 * 1024)
                    require(process.wait() == 0, "native image export failed")
        require(archive.stat().st_size > 0, "empty native image archive")
        with archive.open("rb") as stream:
            archive_sha256 = hashlib.file_digest(stream, "sha256").hexdigest()
        metadata = {
            "schema": "cma-ci-native-image-v1",
            "component": component,
            "source": source,
            "sourceRevision": revision,
            "sourceTree": source_tree,
            "platform": "linux/amd64",
            "imageId": image_id,
            "testedReference": reference,
            "versionLabel": version,
            "runtimeContract": runtime_contract,
            "archiveFormat": "docker-save+gzip",
            "archive": archive.name,
            "archiveSha256": archive_sha256,
            "archiveBytes": archive.stat().st_size,
            "workflow": {
                "repository": repository, "runId": run_id, "runAttempt": attempt,
                "event": event, "ref": ref,
            },
        }
        (destination / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
        print(json.dumps({"component": component, "imageId": image_id, "archiveBytes": metadata["archiveBytes"]}))
    except BaseException:
        # Failed exports cannot leave a seemingly complete transfer bundle.
        shutil.rmtree(destination)
        raise


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("component", choices=("api", "web", "runtime", "runtime-omarchy", "cmaeg"))
    parser.add_argument("revision")
    parser.add_argument("destination", type=Path)
    args = parser.parse_args()
    export(args.component, args.revision, args.destination)
